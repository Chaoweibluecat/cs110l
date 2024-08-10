use std::borrow::{Borrow, BorrowMut};
use std::collections::HashMap;
use std::usize;

use crate::debugger_command::DebuggerCommand;
use crate::dwarf_data::{DwarfData, Error as DwarfError};
use crate::inferior::{Inferior, Status};
use libc::{ptrace, SIGTRAP, WEXITED};
use nix::sys::ptrace;
use nix::sys::signal::Signal;
use nix::sys::wait::WaitPidFlag;
use rustyline::error::ReadlineError;
use rustyline::Editor;

#[derive(Clone, Debug)]
struct Breakpoint {
    addr: usize,
    orig_byte: u8,
}
pub struct Debugger {
    target: String,
    history_path: String,
    readline: Editor<()>,
    inferior: Option<Inferior>,
    debug_data: DwarfData,
    pre_installed_break_points: Vec<usize>,
    break_points: HashMap<usize, Breakpoint>,
}

impl Debugger {
    /// Initializes the debugger.
    pub fn new(target: &str) -> Debugger {
        // TODO (milestone 3): initialize the DwarfData

        let history_path = format!("{}/.deet_history", std::env::var("HOME").unwrap());
        let mut readline = Editor::<()>::new();
        // Attempt to load history from ~/.deet_history if it exists
        let _ = readline.load_history(&history_path);

        let debug_data = match DwarfData::from_file(target) {
            Ok(val) => val,
            Err(DwarfError::ErrorOpeningFile) => {
                println!("Could not open file {}", target);
                std::process::exit(1);
            }
            Err(DwarfError::DwarfFormatError(err)) => {
                println!("Could not debugging symbols from {}: {:?}", target, err);
                std::process::exit(1);
            }
        };
        debug_data.print();

        Debugger {
            target: target.to_string(),
            history_path,
            readline,
            inferior: None,
            debug_data,
            pre_installed_break_points: vec![],
            break_points: HashMap::new(),
        }
    }

    pub fn run(&mut self) {
        loop {
            match self.get_next_command() {
                DebuggerCommand::Run(args) => {
                    // run overrides previous running inferior process
                    if let Some(inf) = self.inferior.as_mut() {
                        inf.kill().expect("kill failed,we fucked up");
                    }
                    if let Some(inferior) = Inferior::new(&self.target, &args) {
                        self.inferior = Some(inferior);
                        self.apply_break_points();
                        println!("{:?}", self.break_points);
                        self.cont_and_handle_ret_status();
                    } else {
                        println!("Error starting subprocess");
                    }
                }
                DebuggerCommand::Quit => {
                    // kill inferior to avoid orphan process
                    if !self.inferior.is_none() {
                        if self.inferior.as_mut().unwrap().kill().is_err() {
                            panic!("kill failed,we fucked up")
                        }
                    }
                    return;
                }
                DebuggerCommand::Continue => {
                    if self.inferior.is_none() {
                        print!("you have to run before continue");
                        return;
                    }
                    let mut inf = self.inferior.as_mut().unwrap();
                    // maybe we can save a syscall
                    let bp = inf.rip();
                    // if we are are at a bp (which actually has been removed by the post processs of wait),
                    // step one instrcution and restore bp
                    if self.break_points.contains_key(&(inf.rip())) {
                        ptrace::step(inf.pid(), None).unwrap();
                        let v1 = inf.wait(None).unwrap();
                        match v1 {
                            Status::Stopped(Signal::SIGTRAP, _) => {
                                // do we need to restore Trap Flag?
                                inf.write_byte(bp, 0xcc).unwrap();
                            }
                            Status::Stopped(sig, _) => {
                                // do we need to restore Trap Flag?
                                {
                                    println!("{:?}", sig)
                                }
                            }
                            Status::Exited(i) => {
                                self.print_status(&Status::Exited(i));
                                return;
                            }
                            // could it be ??
                            _ => {
                                println!("111")
                            }
                        }
                    }
                    self.cont_and_handle_ret_status();
                }
                DebuggerCommand::BackTrace => {
                    let inf = self.inferior.as_mut().unwrap();
                    inf.print_backtrace(&self.debug_data).unwrap();
                }
                DebuggerCommand::Break(str) => {
                    let bp = {
                        if str.starts_with('*') {
                             parse_address(&get_addr_str(&str)) 
                        } else if let Ok(line_num) = str.parse() {
                            self.debug_data.get_addr_for_line(None, line_num)
                        } else  {
                            self.debug_data.get_addr_for_function(None, &str)
                        }
                    };
                    match bp {
                        None => {
                            println!("invalid breakpoint {}", str.as_str());
                        }
                        Some(addr) => {
                            let idx: usize = self.pre_installed_break_points.len();
                            println!("Set breakpoint {} at {:#x}", idx, addr);
                            self.add_break_points(addr);        
                        }
                    }
                }
            }
        }
    }

    /// This function prompts the user to enter a command, and continues re-prompting until the user
    /// enters a valid command. It uses DebuggerCommand::from_tokens to do the command parsing.
    ///
    /// You don't need to read, understand, or modify this function.
    fn get_next_command(&mut self) -> DebuggerCommand {
        loop {
            // Print prompt and get next line of user input
            match self.readline.readline("(deet) ") {
                Err(ReadlineError::Interrupted) => {
                    // User pressed ctrl+c. We're going to ignore it
                    println!("Type \"quit\" to exit");
                }
                Err(ReadlineError::Eof) => {
                    // User pressed ctrl+d, which is the equivalent of "quit" for our purposes
                    return DebuggerCommand::Quit;
                }
                Err(err) => {
                    panic!("Unexpected I/O error: {:?}", err);
                }
                Ok(line) => {
                    if line.trim().len() == 0 {
                        continue;
                    }
                    self.readline.add_history_entry(line.as_str());
                    if let Err(err) = self.readline.save_history(&self.history_path) {
                        println!(
                            "Warning: failed to save history file at {}: {}",
                            self.history_path, err
                        );
                    }
                    let tokens: Vec<&str> = line.split_whitespace().collect();
                    if let Some(cmd) = DebuggerCommand::from_tokens(&tokens) {
                        return cmd;
                    } else {
                        println!("Unrecognized command.");
                    }
                }
            }
        }
    }

    fn cont_and_handle_ret_status(&mut self) {
        match self.inferior.as_mut().unwrap().cont() {
            Ok(status) => {
                self.print_status(&status);
                let inf: &mut Inferior = self.inferior.as_mut().unwrap();
                match status {
                    Status::Stopped(sig, rip) => {
                        // stop at break point?? rewrite the original instruction at bp, at dec rip
                        if self.break_points.contains_key(&(rip - 1)) {
                            // retrieve original instruction
                            let orig_ins = self
                                .break_points
                                .get_key_value(&(rip - 1))
                                .unwrap()
                                .1
                                .orig_byte;
                            // rewrite bp to original instructions
                            inf.write_byte(rip - 1, orig_ins)
                                .expect("rewrite addr failed");
                            // rewrite rip
                            let mut regs = ptrace::getregs(inf.pid()).unwrap();
                            regs.rip -= 1;
                            ptrace::setregs(inf.pid(), regs).expect("rewrite rips failed");
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn print_status(&self, status: &Status) {
        match status {
            Status::Exited(i) => {
                println!("Child exited (status {:?})", i)
            }
            Status::Stopped(i, rip) => {
                println!("Child stopped (status {:?})", i);
                let line = self.debug_data.get_line_from_addr((*rip) as usize);
                let func = self.debug_data.get_function_from_addr((*rip) as usize);
                if line.is_some() && func.is_some() {
                    println!(
                        "{:?}({:?}:{})",
                        func.unwrap(),
                        line.as_ref().unwrap().file,
                        line.as_ref().unwrap().number
                    );
                    println!("rip:{:#x}", *rip);
                }
            }
            _ => {}
        }
    }

    /**
     * 1. if no current program running, add bp to the pre_installed list
     * 2. add bp directly if inferior is running
     */
    fn add_break_points(&mut self, break_point: usize) {
        match self.inferior.as_mut() {
            None => {
                self.pre_installed_break_points.push(break_point);
            }
            Some(inf) => {
                if self.break_points.contains_key(&break_point) {
                    return;
                }
                apply_break_point(inf, &mut self.break_points, break_point);
            }
        }
    }

    /**
     * install all prepared bps
     */
    fn apply_break_points(&mut self) {
        let inf = self.inferior.as_mut().unwrap();
        for ele in &self.pre_installed_break_points {
            apply_break_point(inf, &mut self.break_points, *ele)
        }
    }
}

fn parse_address(addr: &str) -> Option<usize> {
    let addr_without_0x = if addr.to_lowercase().starts_with("0x") {
        &addr[2..]
    } else {
        &addr
    };
    usize::from_str_radix(addr_without_0x, 16).ok()
}

fn apply_break_point(inf: &mut Inferior, map: &mut HashMap<usize, Breakpoint>, addr: usize) {
    let orig_byte = inf
        .write_byte(addr, 0xcc)
        .expect(format!("write byte failed !{}", addr).as_str());
    map.insert(
        addr,
        Breakpoint {
            addr: addr,
            orig_byte,
        },
    );
}

fn get_addr_str(s: &String) -> String {
        let chars: Vec<char> = s.chars().collect();
        let sliced_chars: Vec<char> = chars[1..].to_vec();
        sliced_chars.iter().collect()
}
