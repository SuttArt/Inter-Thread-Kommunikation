#![allow(unused_variables)]

use std::cell::UnsafeCell;
// Wird für Daten mit speziellen Laufzeiten in unsafe Code verwendet
use std::marker::PhantomData;
// Thread-sicherer Referenz-Zähl-Zeiger. "Hält" die Daten und ermöglicht Data-Sharing zwischen Threads
use std::sync::Arc;
// Atomics, Ordering - bestimmt, wie sich Atomics verhalten
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
// Iterator
use std::iter;

/// Producer - sendet/schreibt Daten in gemeinsamen Buffer
pub struct Producer<T: Send> {
	buffer: Arc<Buffer<T>>,
	access_flags: Arc<AccessFlags>,
	_marker: PhantomData<T>,
}

/// Consumer - liest/bekommt Daten aus dem gemeinsamen Buffer
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
	/// Methode, die die Daten in den Buffer schreibt
	/// # Beispiel
	/// ## Senden einer Nachricht:
	/// ```rust:
	/// let (px, cx) = channel();
	/// thread::spawn(move || {
	/// 	px.send("Ping").unwrap();
	/// });
	/// ```
	/// ## Senden von 10 Nachrichten:
	///
	///```rust:
	/// let (px, cx) = channel();
	/// thread::spawn(move || {
	/// 	for i in 0..10 {
	/// 		px.send(i).unwrap();
	/// 	}
	/// });
	/// ```
	pub fn send(&self, mut val: T) -> Result<(), SendError<T>> {
		// Verzögerung in ms. Wird genutzt, damit die send-Funktion den Buffer nicht dauerhaft blockiert.
		let mut backoff = 1;
		loop {
			// Peterson’s Algorithm
			self.access_flags.lock(0);

			// Critical section
			let result = self.buffer.push(val);

			// Exit section
			self.access_flags.unlock(0);

			// Check, ob der Push erfolgreich war
			match result {
				// Erfolgreicher Push, Return
				Ok(_) => return Ok(()),
				// Error
				Err(SendError(returned_val)) => {
					// Check, ob der Consumer noch verfügbar ist
					if self.access_flags.done.load(Ordering::SeqCst) {
						// Stop, wenn der Consumer beendet wurde
						return Err(SendError(returned_val));
					}
					// Andernfalls Item aus dem Fehler extrahieren und erneut versuchen
					val = returned_val;
				}
			}

			// Buffer ist voll, warte und versuche erneut mit exponentiellem Backoff
			std::thread::sleep(std::time::Duration::from_millis(backoff));
			// Backoff darf maximal 100 ms sein
			backoff = (backoff * 2).min(100);
		}
	}
}

impl<T: Send> Drop for Producer<T> {
	/// Drop - Destruktor in Rust.
	/// Wird automatisch aufgerufen, wenn das Objekt out of Scope ist.
	fn drop(&mut self) {
		// Signalisiert, dass keine Items mehr gesendet werden
		self.access_flags.done.store(true, Ordering::SeqCst);
	}
}


impl<T: Send> Consumer<T> {
	/// Methode, die das erste Element aus dem Buffer liest
	/// # Beispiel
	/// ## Lesen einer Nachricht:
	/// ```rust:
	/// let (px, cx) = channel();
	/// println!("recv: {}", cx.recv().unwrap());
	/// ```
	/// ## Lesen von 10 Nachrichten:
	///
	///```rust:
	/// let (px, cx) = channel();
	/// for _ in 0..10 {
	/// 	println!("Got: {}", cx.recv().unwrap());
	/// }
	/// ```
	pub fn recv(&self) -> Result<T, RecvError> {
		loop {
			// Peterson’s Algorithm
			self.access_flags.lock(1);

			// Critical section
			let result = self.buffer.pop();

			// Exit section
			self.access_flags.unlock(1);

			// Return, wenn ein Item existiert
			if let Some(item) = result {
				return Ok(item);
			}

			// Wenn der Buffer leer ist, prüfen, ob der Producer fertig ist
			if self.access_flags.done.load(Ordering::SeqCst) {
				// Keine weiteren Items werden produziert, beenden
				return Err(RecvError);
			}

			// Wenn der Buffer leer ist, yield kurz before retrying
			std::thread::yield_now();
		}
	}
}

impl<T: Send> Drop for Consumer<T> {
	/// Drop - Destruktor in Rust.
	/// Wird automatisch aufgerufen, wenn das Objekt out of Scope ist.
	fn drop(&mut self) {
		// Signalisiert, dass keine Items mehr empfangen werden
		self.access_flags.done.store(true, Ordering::SeqCst);
	}
}

/// Iterator - erstellt einen Iterator für dieses Objekt, sodass danach sowas funktioniert:
///```rust:
/// let (px, cx) = channel();
/// for item in cx {
///     println!("Received (iterator): {}", item);
/// }
/// ```
impl<T: Send> Iterator for Consumer<T> {
	type Item = T;
	fn next(&mut self) -> Option<Self::Item> {
		loop {
			// Peterson’s Algorithm
			self.access_flags.lock(1);

			// Critical section
			let result = self.buffer.pop();
			// Exit section
			self.access_flags.unlock(1);

			// Return, wenn ein Item existiert
			if result.is_some() {
				return result;
			}
			// Wenn der Buffer leer ist, prüfen, ob der Producer fertig ist
			if self.access_flags.done.load(Ordering::SeqCst) {
				// Keine weiteren Items werden produziert, beenden
				// Kein Fehler, da wir Option<Self::Item> zurückgeben
				return None;
			}
			// Wenn der Buffer leer ist, kurz warten, bevor erneut versucht wird
			std::thread::yield_now();
		}
	}
}


/// Ohne Send dürfen wir die Objekte nicht in Thread verschieben
unsafe impl<T: Send> Send for Producer<T> {}
unsafe impl<T: Send> Send for Consumer<T> {}

/// Buffer-Objekt, wird als gemeinsamer Speicher zwischen Producer und Consumer genutzt.
/// Implementiert als Ring-Buffer, sodass unendlich viele Items hinzugefügt werden können, nachdem sie gelesen wurden.
#[derive(Debug)]
pub struct Buffer<T> {
	// Daten im Buffer
	data: UnsafeCell<Vec<Option<T>>>,
	// Index für den Producer, der angibt, wo das letzte Item hinzugefügt wurde
	write_index: UnsafeCell<usize>,
	// Index für den Consumer, der angibt, wo das letzte Item gelesen wurde
	read_index: UnsafeCell<usize>,
	// Gemeinsame Kapazität des Buffers
	capacity: usize,
}

impl<T> Buffer<T> {
	/// new - Konstruktor in Rust
	fn new(capacity: usize) -> Self {
		// iter - Iterator, der Items generiert
		// repeat_with - Wiederholt eine Aktion unendlich oft
		// || None - Lambda-Funktion, auch "Closure" in Rust. Hier fügen wir unendlich oft None in den Buffer ein.
		// .take - begrenzt die repeat_with Funktion auf die Kapazität
		// Mit .collect() speichern wir alles im Buffer
		let buffer = iter::repeat_with(|| None).take(capacity).collect();

		Buffer {
			data: UnsafeCell::new(buffer),
			write_index: UnsafeCell::new(0),
			read_index: UnsafeCell::new(0),
			capacity,
		}
	}

	/// Push in den Buffer
	/// # Beispiel zur Fehlerbehandlung: Elemente hinzufügen
	///```rust:
	///for i in 1..=7 {
	///	match buffer.push(i) {
	///	Ok(_) => println!("Added: {}", i),
	///	Err(SendError(val)) => println!("Buffer full, could not add: {}", SendError(val).0),
	///	}
	///}
	/// ```
	fn push(&self, item: T) -> Result<(), SendError<T>> {
		unsafe {
			let data = &mut *self.data.get();
			let write_index = &mut *self.write_index.get();

			// Wenn am write_index schon ein Item existiert -> Buffer voll -> Fehler
			if data[*write_index].is_some() {
				Err(SendError(item))
			} else {
				// Schreiben in Buffer
				data[*write_index] = Some(item);
				// Ring-Buffer Implementierung
				if *write_index != self.capacity - 1 {
					*write_index += 1;
					if *write_index >= self.capacity {
						*write_index = 0;
					}
				}
				// Ok(()) - Return für Result in Rust
				Ok(())
			}
		}
	}

	/// Pop aus dem Buffer
	/// # Beispiel zur Fehlerbehandlung: Elemente entfernen
	///```rust:
	///    while let Some(value) = buffer.pop() {
	///         println!("Removed: {}", value);
	///     }
	/// ```
	fn pop(&self) -> Option<T> {
		unsafe {
			let data = &mut *self.data.get();
			let read_index = &mut *self.read_index.get();
			let write_index = &mut *self.write_index.get();

			// Wenn wir Item aus dem Buffer lesen/pop können (.take())
			if let Some(item) = data[*read_index].take() {
				// Ring-Buffer Implementierung
				if *read_index != *write_index {
					*read_index += 1;
					if *read_index >= self.capacity {
						*read_index = 0;
					}
				}

				Some(item)
			} else {
				None
			}
		}
	}
}

/// Flags, um den Zugriff auf den gemeinsamen Speicher zu gewährleisten
#[derive(Debug)]
pub struct AccessFlags {
	// Siehe Peterson’s Algorithmus in der Vorlesung
	// 0 für den Producer
	// 1 für den Consumer
	flag: [AtomicBool; 2],
	turn: AtomicUsize,
	// Flag für Übertragungszustand:
	// false - die Daten werden noch übertragen
	// true - Objekte können Destruktoren aufrufen
	done: AtomicBool,
}
impl AccessFlags {
	fn new() -> Self {
		AccessFlags {
			flag: [AtomicBool::new(false), AtomicBool::new(false)],
			turn: AtomicUsize::new(0),
			done: AtomicBool::new(false),
		}
	}

	fn lock(&self, id: usize) {
		let other = 1 - id;
		// Producer/Consumer ist bereit to produce/consume
		self.flag[id].store(true, Ordering::SeqCst);
		// Erlauben producer/consumer zu produce/consume
		self.turn.store(other, Ordering::SeqCst);
		while self.flag[other].load(Ordering::SeqCst) && self.turn.load(Ordering::SeqCst) == other {
			// Warten für producer/consumer to finish
			// Producer/Consumer wartet,ob consumer/producer ist fertig, und jetzt ist consumer/producer an der Reihe
		}
	}

	fn unlock(&self, id: usize) {
		// Producer/Consumer ist nicht mehr im kritischen Bereich
		self.flag[id].store(false, Ordering::SeqCst);
	}
}

/// Channel, Funktion, die den Buffer, Producer und Consumer erstellt
pub fn channel<T: Send>() -> (Producer<T>, Consumer<T>) {
	// Kapazität des Buffers. Kann jede beliebige Zahl sein, da wir einen Ring-Buffer verwenden
	const CAPACITY: usize = 1000;
	// Buffer initialisieren
	// Buffer in Arc umwickeln, damit er von beiden Objekten genutzt werden kann
	let shared_buffer = Arc::new(Buffer::new(CAPACITY));

	// Flags in Arc umwickeln, damit sie von beiden Objekten genutzt werden können
	let shared_access_flags = Arc::new(AccessFlags::new());

	// Initialisieren und Rückgabe von Producer und Consumer
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
