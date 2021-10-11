use std::fs::File;
use std::path::PathBuf;
use std::io::Write;
use std::time::Duration;

pub struct CsvLogger {
	file: Option<File>,
	file_path: PathBuf,
	buffers: Vec<Vec<Duration>>,
	rows_written: usize,
	num_targets: usize,
	
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
		
		let mut file = File::create(&p).unwrap();
		file.write_all(b"Time").unwrap();
		
		for i in 0..num_targets {
			if i == 0 {
				file.write_all(b", Gateway").unwrap();
				continue;
			}
			
			file.write_all(&format!(", ISP Hop {}",i).as_bytes()).unwrap();
		}
		
		file.write_all(b"\n").unwrap();
		
		CsvLogger {
			file: Some(file),
			file_path: p,
			buffers: vec![Vec::new();num_targets],
			rows_written: 0,
			num_targets,
		}
	}
	
	pub fn log(&mut self, host_id: usize, value: Duration) {
		assert!(host_id < self.num_targets);
		self.buffers[host_id].push(value);
		
		let mut row_complete = true;
		for buf in &self.buffers {
			if buf.len() <= self.rows_written { row_complete = false; }
		}
		if !row_complete { return; }
		
		// We don't use floating point types here since they cause ugly presicion errors.
		let time_decisecs = self.rows_written * 2;
		let lower = time_decisecs % 10;
		let upper = time_decisecs / 10;
		self.file.as_mut().unwrap().write_all(&format!("{}.{},", upper, lower).as_bytes()).unwrap();
		
		for buf in &self.buffers {
			let duration = buf[self.rows_written];
			self.file.as_mut().unwrap().write_all(&format!("{}",duration.as_millis()).as_bytes()).unwrap();
			self.file.as_mut().unwrap().write_all(b",").unwrap();
		}
		
		self.file.as_mut().unwrap().write_all(b"\n").unwrap();
		self.file.as_mut().unwrap().flush().unwrap();
		
		self.rows_written += 1;
	}
}
impl Drop for CsvLogger {
	fn drop(&mut self) {
		for buf in &mut self.buffers {
			buf.sort_unstable();
		}
		
		let mut i : u32 = 1;
		let mut tmp_p = PathBuf::new();
		loop {
			tmp_p.push(format!("ping.tmp{}.csv",i));
			if !tmp_p.exists() { break; }
			tmp_p.pop();
			i += 1;
		}
		let mut new_file = File::create(&tmp_p).unwrap();
		
		new_file.write_all(b",").unwrap();
		for buf in &self.buffers {
			let sum : u128 = buf.iter().map(|d| d.as_millis()).sum();
			new_file.write_all(format!("{},",sum/(buf.len() as u128)).as_bytes()).unwrap();
		}
		new_file.write_all(b"Average\n").unwrap();
		
		new_file.write_all(b",").unwrap();
		for buf in &self.buffers {
			let value = buf[((buf.len() as f32)*0.95).floor() as usize].as_millis();
			new_file.write_all(&format!("{},",value).as_bytes()).unwrap();
		}
		new_file.write_all(b"95th percentile\n").unwrap();
		
		new_file.write_all(b",").unwrap();
		for buf in &self.buffers {
			let value = buf[((buf.len() as f32)*0.99).floor() as usize].as_millis();
			new_file.write_all(&format!("{},",value).as_bytes()).unwrap();
		}
		new_file.write_all(b"99th percentile\n").unwrap();
		
		new_file.write_all(b",").unwrap();
		for _ in 0..self.num_targets {
			new_file.write_all(b",").unwrap();
		}
		new_file.write_all(b"\n").unwrap();
		new_file.write_all(b",").unwrap();
		for _ in 0..self.num_targets {
			new_file.write_all(b",").unwrap();
		}
		new_file.write_all(b"\n").unwrap();
		
		self.file.take();
		let mut main_file = File::open(&self.file_path).unwrap();
		std::io::copy(&mut main_file, &mut new_file).unwrap();
		std::mem::drop(main_file);
		std::mem::drop(new_file);
		std::fs::remove_file(&self.file_path).unwrap();
		std::fs::rename(tmp_p, &self.file_path).unwrap();
	}
}
