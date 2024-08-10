use libc::WUNTRACED;
use nix::sys::ptrace;
use nix::sys::signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;

use crate::dwarf_data::DwarfData;

pub enum Status {
    /// Indicates inferior stopped. Contains the signal that stopped the process, as well as the
    /// current instruction pointer that it is stopped at.
    Stopped(signal::Signal, usize),

    /// Indicates inferior exited normally. Contains the exit status code.
    Exited(i32),

    /// Indicates the inferior exited due to a signal. Contains the signal that killed the
    /// process.
    Signaled(signal::Signal),
}

/// This function calls ptrace with PTRACE_TRACEME to enable debugging on a process. You should use
/// pre_exec with Command to call this in the child process.
fn child_traceme() -> Result<(), std::io::Error> {
    ptrace::traceme().or(Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "ptrace TRACEME failed",
    )))
}

pub struct Inferior {
    child: Child,
}

impl Inferior {
    /// Attempts to start a new inferior process. Returns Some(Inferior) if successful, or None if
    /// an error is encountered.
    pub fn new(target: &str, args: &Vec<String>) -> Option<Inferior> {
        // let command = Command::new(target).args(args);
        let mut command = Command::new(target);
        command.args(args);
        unsafe {
            command.pre_exec(child_traceme);
        }
        let child = command.spawn().expect("failed to spawn inferior");
        let res = Inferior { child };
        let _ = res.wait(None).expect("sig1");
        Some(res)
    }

    /// Returns the pid of this inferior.
    pub fn pid(&self) -> Pid {
        nix::unistd::Pid::from_raw(self.child.id() as i32)
    }

    pub fn kill(&mut self) -> Result<Status, nix::Error> {
        println!("Killing running inferior (pid {})", self.child.id());
        match self.child.kill() {
            Ok(_) => self.wait(None),
            _ => {
                panic!("kill error:")
            }
        }
    }

    //continue: a ptrace:cont followed by a wait
    pub fn cont(&self) -> Result<Status, nix::Error> {
        ptrace::cont(self.pid(), None)?;
        self.wait(None)
    }

    /// Calls waitpid on this inferior and returns a Status to indicate the state of the process
    /// after the waitpid call.
    pub fn wait(&self, options: Option<WaitPidFlag>) -> Result<Status, nix::Error> {
        Ok(match waitpid(self.pid(), options)? {
            WaitStatus::Exited(_pid, exit_code) => Status::Exited(exit_code),
            WaitStatus::Signaled(_pid, signal, _core_dumped) => Status::Signaled(signal),
            WaitStatus::Stopped(_pid, signal) => {
                let regs = ptrace::getregs(self.pid())?;
                Status::Stopped(signal, regs.rip as usize)
            }
            other => panic!("waitpid returned unexpected status: {:?}", other),
        })
    }
    pub fn read(&self, address: ptrace::AddressType) -> usize {
        ptrace::read(self.pid(), address).unwrap() as usize
    }

    pub fn rip(&self) -> usize {
         ptrace::getregs(self.pid()).unwrap().rip as usize
    }

    pub fn print_backtrace(&self, debug_data: &DwarfData) -> Result<(), nix::Error> {
        let regs = ptrace::getregs(self.pid())?;
        // start backtrace, at first we can directly read from regs
        let mut rip = regs.rip as usize;
        let mut rbp = regs.rbp as usize;
        loop {
            let line = debug_data.get_line_from_addr(rip as usize).unwrap();
            let func = debug_data.get_function_from_addr(rip as usize).unwrap();
            println!("{:?}({:?}:{})", func, line.file, line.number);
            if func == "main" {
                break;
            }
            // then we have to read from stack frame; notice rbp now is only a address
            // *rbp =>  saved previous rbp
            // *(rbp+8) =>  saved previous rip
            rip = self.read((rbp + 8) as ptrace::AddressType);
            rbp = self.read(rbp as ptrace::AddressType)
        }
        Ok(())
    }
    /**
     * write a single bytes to the dst inferior process address
     * ptrace only supports rw a full 8 bytes into a long
     * so a bit of bit tricks is needed here
     */
    pub fn write_byte(&mut self, addr: usize, val: u8) -> Result<u8, nix::Error> {
        let aligned_addr = align_addr_to_word(addr);
        let byte_offset = addr - aligned_addr;
        let word = ptrace::read(self.pid(), aligned_addr as ptrace::AddressType)? as u64;
        let orig_byte = (word >> 8 * byte_offset) & 0xff;
        let masked_word = word & !(0xff << 8 * byte_offset);
        let updated_word = masked_word | ((val as u64) << 8 * byte_offset);
        ptrace::write(
            self.pid(),
            aligned_addr as ptrace::AddressType,
            updated_word as *mut std::ffi::c_void,
        )?;
        Ok(orig_byte as u8)
    }
}

fn align_addr_to_word(addr: usize) -> usize {
    addr & (-(size_of::<usize>() as isize) as usize)
}
