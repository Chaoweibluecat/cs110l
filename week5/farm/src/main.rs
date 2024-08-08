use std::{ collections::VecDeque, thread::Thread };
#[allow(unused_imports)]
use std::sync::{ Arc, Mutex };
use std::time::Instant;
#[allow(unused_imports)]
use std::{ env, process, thread };

/// Determines whether a number is prime. This function is taken from CS 110 factor.py.
///
/// You don't need to read or understand this code.
#[allow(dead_code)]
fn is_prime(num: u32) -> bool {
    if num <= 1 {
        return false;
    }
    for factor in 2..(num as f64).sqrt().floor() as u32 {
        if num % factor == 0 {
            return false;
        }
    }
    true
}

/// Determines the prime factors of a number and prints them to stdout. This function is taken
/// from CS 110 factor.py.
///
/// You don't need to read or understand this code.
#[allow(dead_code)]
fn factor_number(num: u32) {
    let start = Instant::now();

    if num == 1 || is_prime(num) {
        println!("{} = {} [time: {:?}]", num, num, start.elapsed());
        return;
    }

    let mut factors = Vec::new();
    let mut curr_num = num;
    for factor in 2..num {
        while curr_num % factor == 0 {
            factors.push(factor);
            curr_num /= factor;
        }
    }
    factors.sort();
    let factors_str = factors
        .into_iter()
        .map(|f| f.to_string())
        .collect::<Vec<String>>()
        .join(" * ");
    println!("{} = {} [time: {:?}]", num, factors_str, start.elapsed());
}

/// Returns a list of numbers supplied via argv.
#[allow(dead_code)]
fn get_input_numbers() -> VecDeque<u32> {
    let mut numbers = VecDeque::new();
    for arg in env::args().skip(1) {
        if let Ok(val) = arg.parse::<u32>() {
            numbers.push_back(val);
        } else {
            println!("{} is not a valid number", arg);
            process::exit(1);
        }
    }
    numbers
}

fn main() {
    let num_threads = num_cpus::get();
    println!("Farm starting on {} CPUs", num_threads);
    let start = Instant::now();
    // let numbers = get_input_numbers();
    let numbers = vec![
        379123821,
        1283712930,
        123871231,
        1238172301,
        128371293,
        781236812,
        126491123,
        129371923,
        1248917249
    ];
    let mut threads = vec![];
    let deque_arc = Arc::new(Mutex::new(numbers));
    for _ in 0..num_threads {
        let local_arc = deque_arc.clone();
        threads.push(
            // 1.move arc from iterator to closure
            thread::spawn(move || {
                // use expression to pop value,
                // by the end of expression, mutex drop and unlocked
                let num: Option<u32> = {
                    let mut numbers = local_arc.lock().unwrap();
                    (*numbers).pop()
                };
                // use if let to factor
                if let Some(n) = num {
                    factor_number(n);
                }
            })
        );
    }
    for handle in threads {
        handle.join().unwrap(); // 等待线程结束
    }

    println!("Total execution time: {:?}", start.elapsed());
}
