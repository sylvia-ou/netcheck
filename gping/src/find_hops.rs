//TODO:
//Support for MacOS users

use std::process::{Command, Child, Stdio, ChildStdout};
use std::io::{BufReader, BufRead};
use dns_lookup::lookup_host;

struct TracertIter {
    trace_route : Child,
    trace_output : BufReader<ChildStdout>
}

impl TracertIter {
    fn new() -> TracertIter {
        let mut trace = if cfg!(target_os = "windows") {
            Command::new("cmd")
                .args(&["/C", "tracert -d google.com"])
                .stdout(Stdio::piped())
                .spawn()
                .expect("failed to execute process")
        } else {
            Command::new("sh")
                .arg("-c")
                .arg("traceroute -n google.com")
                .stdout(Stdio::piped())
                .spawn()
                .expect("failed to execute process")
        };
        let mut output = BufReader::new(trace.stdout.take().unwrap());
        
        // skip unimportant lines
        let mut junk = Vec::new();
        
        if cfg!(target_os = "windows")
        {
            //4 junk lines in windows
            for _ in 0..4
            {
                output.read_until(b'\n',&mut junk).unwrap();
                junk.clear();
            }
        } else
        {
            //1 junk line in linux/MacOS
            output.read_until(b'\n',&mut junk).unwrap();
            junk.clear();
        }
        
        
        TracertIter{trace_route: trace, trace_output: output}
    }
}

impl Iterator for TracertIter {
    type Item = Option<String>;
    
    // Some(None) indicates that the hop didn't respond
    fn next(&mut self) -> Option<Self::Item> {
        let mut line_raw = Vec::new();
        let len = self.trace_output.read_until(b'\n',&mut line_raw).unwrap();
        if len == 0 { return None; }
        let line = String::from_utf8_lossy(&line_raw).into_owned();
        
        let hop_addr: &str;

        if cfg!(target_os = "windows")
        {
            hop_addr = if let Some(a) = line.split_whitespace().nth(7) {
            a
            } else {
                return Some(None);
            };
        } else
        {
            hop_addr = if let Some(a) = line.split_whitespace().nth(1) {
            a
            } else {
                return Some(None);
            };
        }
        
        // hop_addr might be a localized error message (eg. a timeout) we try to
        // do a lookup to test this
        if lookup_host(hop_addr).is_err() { return Some(None); }
        Some(Some(hop_addr.to_owned()))
    }
}

// The first host is the first responding address returned by tracert.
// The 2nd and 3rd hosts are the next two _public_ hosts returned by tracert.
// non-responing hosts will be skipped.
pub fn get_desired_hops() -> [String;3] {
    let mut iter = TracertIter::new();
    
    let first = loop {
        let host_maybe = if let Some(x) = iter.next() { x } else { panic!("unexpected end of tracert output"); };
        if host_maybe.is_some() { break host_maybe.unwrap(); }
    };
    
    let mut public_ips = Vec::with_capacity(2);
    for host_maybe in iter {
        if host_maybe.is_none() { continue; }
        let host = host_maybe.unwrap();
        if !lookup_host(&host).unwrap()[0].is_global() { continue; }
        public_ips.push(host);
        if public_ips.len() == 2 { break; }
    }
    if public_ips.len() < 2 { panic!("unexpected end of tracert output"); }
    
    [first, public_ips[0].clone(), public_ips[1].clone()]
}