use std::fs::File;
use std::path::PathBuf;
use std::io::Write;
use std::time::Duration;

pub struct CsvLogger {
	file: File,
	buffers: Vec<Vec<Option<Duration>>>,
	rows_written: usize,
	num_targets: usize
}
impl CsvLogger {
	pub fn new(num_targets: usize) -> Self {
		let mut i : u32 = 1;
		let mut p = PathBuf::new();
		loop {
			p.push(format!("ping{}.csv",i));
			if !p.exists() { break; }
			p.pop();
			i += 1;
		}
		
		CsvLogger {
			file: File::create(p).unwrap(),
			buffers: vec![Vec::new();num_targets],
			rows_written: 0,
			num_targets,
		}
	}
	
	pub fn log(&mut self, host_id: usize, value: Option<Duration>) {
		assert!(host_id < self.num_targets);
		self.buffers[host_id].push(value);
		
		let mut row_complete = true;
		for buf in &self.buffers {
			if buf.len() <= self.rows_written { row_complete = false; }
		}
		if !row_complete { return; }
		
		for buf in &self.buffers {
			if let Some(duration) = buf[self.rows_written].as_ref() {
				self.file.write_all(format!("{}",duration.as_millis()).as_bytes()).unwrap();
			} else {
				self.file.write_all(b"null").unwrap();
			}
			self.file.write_all(b",").unwrap();
		}
		
		self.file.write_all(b"\n").unwrap();
		self.file.flush().unwrap();
		
		self.rows_written += 1;
	}
}
impl Drop for CsvLogger {
	fn drop(&mut self) {
		for buf in &mut self.buffers {
			buf.retain(|d| d.is_some());
			buf.sort_unstable();
		}
		
		
		for _ in 0..self.num_targets {
			self.file.write_all(b",").unwrap();
		}
		self.file.write_all(b"\n").unwrap();
		for _ in 0..self.num_targets {
			self.file.write_all(b",").unwrap();
		}
		self.file.write_all(b"\n").unwrap();
		
		for buf in &self.buffers {
			let sum : u128 = buf.iter().map(|d| d.unwrap().as_millis()).sum();
			self.file.write_all(format!("{},",sum/(buf.len() as u128)).as_bytes()).unwrap();
		}
		self.file.write_all(b"Average\n").unwrap();
		
		
		for buf in &self.buffers {
			let value = buf[((buf.len() as f32)*0.95).floor() as usize].unwrap().as_millis();
			self.file.write_all(&format!("{},",value).as_bytes()).unwrap();
		}
		self.file.write_all(b"95th percentile\n").unwrap();
		
		for buf in &self.buffers {
			let value = buf[((buf.len() as f32)*0.99).floor() as usize].unwrap().as_millis();
			self.file.write_all(&format!("{},",value).as_bytes()).unwrap();
		}
		self.file.write_all(b"99th percentile\n").unwrap();
		
		self.file.flush().unwrap();
	}
}
