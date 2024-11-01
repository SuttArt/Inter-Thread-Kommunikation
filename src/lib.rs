#![allow(unused_variables)]

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::iter;

pub struct Producer<T: Send> {
	buffer: Arc<Buffer<T>>,
	access_flags: Arc<AccessFlags>,
	_marker: PhantomData<T>,
}

pub struct Consumer<T: Send> {
	buffer: Arc<Buffer<T>>,
	access_flags: Arc<AccessFlags>,
	_marker: PhantomData<T>,
}


#[derive(Debug)]
pub struct SendError<T>(pub T);

#[derive(Debug)]
pub struct RecvError;

impl<T: Send> Producer<T> {
	pub fn send(&self, mut val: T) -> Result<(), SendError<T>> {
		let mut backoff = 1; // Initial backoff delay in milliseconds
		loop {
			// Peterson’s Algorithm
			self.access_flags.prod_flag.store(true, Ordering::SeqCst);
			self.access_flags.turn.store(1, Ordering::SeqCst);

			while self.access_flags.turn.load(Ordering::SeqCst) == 1 {
				if !self.access_flags.cons_flag.load(Ordering::SeqCst) {
					break;
				}
			}

			// Critical section
			let result = unsafe {
				let buffer_ptr = Arc::as_ptr(&self.buffer) as *mut Buffer<T>;
				(*buffer_ptr).push(val)
			};

			// Exit section
			self.access_flags.prod_flag.store(false, Ordering::SeqCst);

			// Check if push was successful
			match result {
				Ok(_) => return Ok(()), // Successful push, return
				Err(SendError(returned_val)) => {
					// Check if the consumer is still available
					if self.access_flags.done.load(Ordering::SeqCst) {
						return Err(SendError(returned_val)); // Stop if the consumer is dropped
					}
					// Otherwise, extract the item and retry
					val = returned_val;
				}
			}

			// Buffer is full, wait and retry with exponential backoff
			std::thread::sleep(std::time::Duration::from_millis(backoff));
			backoff = (backoff * 2).min(100); // Cap backoff at 100 ms
		}
	}
}

impl<T: Send> Drop for Producer<T> {
	fn drop(&mut self) {
		// Signal that no more items will be sent
		self.access_flags.done.store(true, Ordering::SeqCst);
	}
}


impl<T: Send> Consumer<T> {
	pub fn recv(&self) -> Result<T, RecvError> {
		loop {
			// Peterson’s Algorithm
			self.access_flags.cons_flag.store(true, Ordering::SeqCst);
			self.access_flags.turn.store(0, Ordering::SeqCst); // Give turn to producer

			while self.access_flags.turn.load(Ordering::SeqCst) == 0 {
				if !self.access_flags.prod_flag.load(Ordering::SeqCst) {
					break;
				}
			}

			// Critical section
			let result = unsafe {
				let buffer_ptr = Arc::as_ptr(&self.buffer) as *mut Buffer<T>;
				(*buffer_ptr).pop()
			};

			// Exit section
			self.access_flags.cons_flag.store(false, Ordering::SeqCst);

			// Return the item if available
			if let Some(item) = result {
				return Ok(item);
			}

			// If the buffer is empty, check if producer is done
			if self.access_flags.done.load(Ordering::SeqCst) {
				return Err(RecvError); // No more items will be produced, exit
			}

			// If the buffer is empty, yield briefly before retrying
			std::thread::yield_now();
		}
	}
}

impl<T: Send> Drop for Consumer<T> {
	fn drop(&mut self) {
		// Signal that no more items will be sent
		self.access_flags.done.store(true, Ordering::SeqCst);
	}
}

impl<T: Send> Iterator for Consumer<T> {
	type Item = T;
	fn next(&mut self) -> Option<Self::Item> {
		loop {
			// Peterson’s Algorithm
			self.access_flags.cons_flag.store(true, Ordering::SeqCst);
			self.access_flags.turn.store(0, Ordering::SeqCst); // Give turn to producer

			while self.access_flags.turn.load(Ordering::SeqCst) == 0 {
				if !self.access_flags.prod_flag.load(Ordering::SeqCst) {
					break;
				}
			}

			// Critical section
			let result = unsafe {
				let buffer_ptr = Arc::as_ptr(&self.buffer) as *mut Buffer<T>;
				(*buffer_ptr).pop()
			};

			// Exit section
			self.access_flags.cons_flag.store(false, Ordering::SeqCst);

			// Check if an item was available
			if result.is_some() {
				return result;
			}

			// If the buffer is empty, check if producer is done
			if self.access_flags.done.load(Ordering::SeqCst) {
				return None; // No more items will be produced, exit
			}

			// If the buffer is empty, yield briefly to avoid busy waiting
			std::thread::yield_now();
		}
	}
}


unsafe impl<T: Send> Send for Producer<T> {}
unsafe impl<T: Send> Send for Consumer<T> {}

#[derive(Debug)]
pub struct Buffer<T> {
	data: Vec<Option<T>>,
	write_index: usize,
	read_index: usize,
	capacity: usize,
}

impl<T> Buffer<T> {
	fn new(capacity: usize) -> Self {
		let buffer = iter::repeat_with(|| None).take(capacity).collect();

		Buffer {
			data: buffer,
			write_index: 0,
			read_index: 0,
			capacity,
		}
	}

	fn push(&mut self, item: T) -> Result<(), SendError<T>> {
		if self.data[self.write_index].is_some() {
			Err(SendError(item))
		} else {
			self.data[self.write_index] = Some(item);
			if self.write_index != self.capacity - 1 {
				self.write_index += 1;
				if self.write_index >= self.capacity {
					self.write_index = 0;
				}
			}
			Ok(())
		}
	}

	fn pop(&mut self) -> Option<T> {
		if let Some(item) = self.data[self.read_index].take() {
			if self.read_index != self.write_index {
				self.read_index += 1;
				if self.read_index >= self.capacity {
					self.read_index = 0;
				}
			}
			Some(item)
		} else {
			None
		}
	}
}

#[derive(Debug)]
pub struct AccessFlags {
	prod_flag: AtomicBool,
	cons_flag: AtomicBool,
	turn: AtomicUsize,
	done: AtomicBool,
}
pub fn channel<T: Send>() -> (Producer<T>, Consumer<T>) {
	const CAPACITY: usize = 1000;
	let buffer = Buffer::new(CAPACITY);
	let shared_buffer = Arc::new(buffer);

	// Peterson’s algorithm
	let access_flags = AccessFlags {
		prod_flag: AtomicBool::new(false),
		cons_flag: AtomicBool::new(false),
		turn: AtomicUsize::new(0),
		done: AtomicBool::new(false),
	};
	let shared_access_flags = Arc::new(access_flags);

	(Producer {
		buffer: Arc::clone(&shared_buffer),
		access_flags: Arc::clone(&shared_access_flags),
		_marker: Default::default(),
	},
	 Consumer {
		 buffer: Arc::clone(&shared_buffer),
		 access_flags: Arc::clone(&shared_access_flags),
		 _marker: Default::default(),
	 })
}

// vorimplementierte Testsuite; bei Bedarf erweitern!

#[cfg(test)]
mod tests {
	use std::{
		collections::HashSet,
		sync::{
			Mutex,
			LazyLock
		},
		thread,
	};
	
	use super::*;
	
	static FOO_SET: LazyLock<Mutex<HashSet<i32>>> = LazyLock::new(|| {
		Mutex::new(HashSet::new())
	});
	
	#[derive(Debug)]
	struct Foo(i32);
	
	impl Foo {
		fn new(key: i32) -> Self {
			assert!(
				FOO_SET.lock().unwrap().insert(key),
				"double initialisation of element {}", key
			);
			Foo(key)
		}
	}
	
	impl Drop for Foo {
		fn drop(&mut self) {
			assert!(
				FOO_SET.lock().unwrap().remove(&self.0),
				"double free of element {}", self.0
			);
		}
	}
	
	// range of elements to be moved across the channel during testing
	const ELEMS: std::ops::Range<i32> = 0..1000;
	
	#[test]
	fn unused_elements_are_dropped() {
		for i in 0..100 {
			let (px, cx) = channel();
			let handle = thread::spawn(move ||
				for i in 0.. {
					if px.send(Foo::new(i)).is_err() {
						return;
					}
				}
			);
			
			for _ in 0..i {
				cx.recv().unwrap();
			}
			
			drop(cx);
			
			assert!(handle.join().is_ok());
			
			let map = FOO_SET.lock().unwrap();
			if !map.is_empty() {
				panic!("FOO_MAP not empty: {:?}", *map);
			}
		}
	}
	
	#[test]
	fn elements_arrive_ordered() {
		let (px, cx) = channel();
		
		thread::spawn(move || {
			for i in ELEMS {
				px.send(i).unwrap();
			}
		});
		
		for i in ELEMS {
			assert_eq!(i, cx.recv().unwrap());
		}
		
		assert!(cx.recv().is_err());
	}
	
	#[test]
	fn all_elements_arrive() {
		for _ in 0..100 {
			let (px, cx) = channel();
			let handle = thread::spawn(move || {
				let mut count = 0;
				
				while cx.recv().is_ok() {
					count += 1;
				}
				
				count
			});
			
			thread::spawn(move || {
				for i in ELEMS {
					px.send(i).unwrap();
				}
			});
			
			match handle.join() {
				Ok(count) => assert_eq!(count, ELEMS.len()),
				Err(_) => panic!("Error: join() returned Err")
			}
		}
	}
}
