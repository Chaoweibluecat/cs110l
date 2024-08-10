use crossbeam_channel::{self, Receiver, Sender};
use std::{thread::{self, Thread}, time};
pub struct InputUnit<T> {
    idx : usize,
    item : T
}
pub struct ResultUnit<U> {
    idx : usize,
    res : U
}

fn parallel_map<T, U, F>(mut input_vec: Vec<T>, num_threads: usize, f: F) -> Vec<U>
where
    F: FnOnce(T) -> U + Send + Copy + 'static,
    T: Send + 'static,
    U: Send + 'static + Default,
{
    // countDownLatch
    let len = input_vec.len();
    let mut output_vec: Vec<U> = Vec::with_capacity(len);
    // 主线程给子进程send T, block在recvT上
    let (input_sender,input_receiver):(Sender<InputUnit<T>>, Receiver<InputUnit<T>>)  = crossbeam_channel::bounded(input_vec.len());
    let (res_sender,res_receiver):(Sender<ResultUnit<U>>, Receiver<ResultUnit<U>>)  = crossbeam_channel::bounded(input_vec.len());

    for _ in 0..num_threads {
        // make local copy of channel endpoint!
        let recvr = input_receiver.clone();
        let sender = res_sender.clone();
        // move channel endpoint to thread 
        thread::spawn(move || {
            while let Ok(rec) = recvr.recv() {
                let res = f(rec.item);
                sender.send(ResultUnit{idx:rec.idx, res}).expect("send message failed");
            }
            // 使用完之后无需要手动drop channel;move进来的clone生命周期自然结束
        });
    }
    drop(input_receiver);
    drop(res_sender);
    for (i, ele) in input_vec.into_iter().enumerate() {
        input_sender.send(InputUnit {idx:i, item:(ele)}).expect("send success");
    }
    // 主线程及时关闭输入通道,避免子线程block在recv上
    drop(input_sender);
    output_vec.reserve(len);
    unsafe {
        output_vec.set_len(len);
    }
    while let Ok(res) = res_receiver.recv() {
        output_vec[res.idx] = res.res;
    }
    output_vec
}

fn main() {
    let v = vec![6, 7, 8, 9, 10, 1, 2, 3, 4, 5, 12, 18, 11, 5, 20];
    let squares = parallel_map(v, 10, |num| {
        println!("{} squared is {}", num, num * num);
        thread::sleep(time::Duration::from_millis(500));
        num * num
    });
    println!("squares: {:?}", squares);
}
