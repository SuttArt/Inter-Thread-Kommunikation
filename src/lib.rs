#![allow(unused_variables)]

use std::marker::PhantomData;

pub struct Producer<T: Send> {
	// TODO: fill with life
	_marker: PhantomData<T>
}
pub struct Consumer<T: Send> {
	// TODO: fill with life
	_marker: PhantomData<T>
}

#[derive(Debug)]
pub struct SendError<T>(pub T);

#[derive(Debug)]
pub struct RecvError;

impl<T: Send> Producer<T> {
	pub fn send(&self, val: T) -> Result<(), SendError<T>> {
		todo!("fill with life")
	}
}

impl<T: Send> Consumer<T> {
	pub fn recv(&self) -> Result<T, RecvError> {
		todo!("fill with life")
	}
}

impl<T: Send> Iterator for Consumer<T> {
	type Item = T;
	fn next(&mut self) -> Option<Self::Item> {
		todo!("fill with life")
	}
}

unsafe impl<T: Send> Send for Producer<T> {}
unsafe impl<T: Send> Send for Consumer<T> {}

pub fn channel<T: Send>() -> (Producer<T>, Consumer<T>) {
	todo!("construct the channel")
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
