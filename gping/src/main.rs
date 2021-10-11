#![feature(ip)]

use crate::plot_data::PlotData;
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use crossterm::event::{KeyEvent, KeyModifiers};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use dns_lookup::lookup_host;
use pinger::{ping, PingResult};
use std::io;
use std::iter;
use std::net::IpAddr;
use std::ops::Add;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{mpsc, Arc};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::collections::VecDeque;
use structopt::StructOpt;
use tui::backend::CrosstermBackend;
use tui::layout::{Constraint, Direction, Layout};
use tui::style::{Color, Style};
use tui::text::Span;
use tui::widgets::{Axis, Block, Borders, Chart, Dataset};
use tui::Terminal;
mod plot_data;
mod find_hops;
mod log;

const HOP_COLORS : [Color;3] = [
    Color::White,
    Color::Cyan,
    Color::LightMagenta,
];

#[derive(Debug, StructOpt)]
#[structopt(name = "gping", about = "Ping, but with a graph.")]
struct Args {
    #[structopt(
        long,
        help = "Graph the execution time for a list of commands rather than pinging hosts"
    )]
    cmd: bool,
    #[structopt(
        short = "n",
        long,
        help = "Watch interval seconds (provide partial seconds like '0.5')",
        default_value = "0.5"
    )]
    watch_interval: f32,
    #[structopt(
        help = "Hosts or IPs to ping, or commands to run if --cmd is provided."
    )]
    hosts_or_commands: Vec<String>,
    #[structopt(
        short,
        long,
        default_value = "30",
        help = "Determines the number of seconds to display in the graph."
    )]
    buffer: u64,
    /// Resolve ping targets to IPv4 address
    #[structopt(short = "4", conflicts_with = "ipv6")]
    ipv4: bool,
    /// Resolve ping targets to IPv6 address
    #[structopt(short = "6", conflicts_with = "ipv4")]
    ipv6: bool,
    
    #[structopt(short = "s", long, help = "Uses dot characters instead of braille. Enabled by default on Windows.")]
    simple_graphics: bool,
}

struct App {
    data: Vec<PlotData>,
    display_interval: chrono::Duration,
    started: chrono::DateTime<Local>,
}

impl App {
    fn new(data: Vec<PlotData>, buffer: u64) -> Self {
        App {
            data,
            display_interval: chrono::Duration::from_std(Duration::from_secs(buffer)).unwrap(),
            started: Local::now(),
        }
    }

    fn update(&mut self, host_idx: usize, item: Duration) {
        let host = &mut self.data[host_idx];
        host.update(item);
    }

    fn y_axis_bounds(&self) -> [f64; 2] {
        // Find the Y axis bounds for our chart.
        // This is trickier than the x-axis. We iterate through all our PlotData structs
        // and find the min/max of all the values. Then we add a 10% buffer to them.
        let iter = self
            .data
            .iter()
            .map(|b| b.data.as_slice())
            .flatten()
            .map(|v| v.1);
        let min = iter.clone().fold(f64::INFINITY, |a, b| a.min(b));
        let max = iter.fold(0f64, |a, b| a.max(b));
        // Add a 10% buffer to the top and bottom
        let max_10_percent = (max * 10_f64) / 100_f64;
        let min_10_percent = (min * 10_f64) / 100_f64;
        [min - min_10_percent, max + max_10_percent]
    }

    fn x_axis_bounds(&self) -> [f64; 2] {
        let now = Local::now();
        let now_idx;
        let before_idx;
        if (now - self.started) < self.display_interval {
            now_idx = (self.started + self.display_interval).timestamp_millis() as f64 / 1_000f64;
            before_idx = self.started.timestamp_millis() as f64 / 1_000f64;
        } else {
            now_idx = now.timestamp_millis() as f64 / 1_000f64;
            let before = now - self.display_interval;
            before_idx = before.timestamp_millis() as f64 / 1_000f64;
        }

        [before_idx, now_idx]
    }

    fn x_axis_labels(&self, bounds: [f64; 2]) -> Vec<Span> {
        let lower_utc = NaiveDateTime::from_timestamp(bounds[0] as i64, 0);
        let upper_utc = NaiveDateTime::from_timestamp(bounds[1] as i64, 0);
        let lower = Local::from_utc_datetime(&Local, &lower_utc);
        let upper = Local::from_utc_datetime(&Local, &upper_utc);
        let diff = (upper - lower) / 2;
        let midpoint = lower + diff;
        return vec![
            Span::raw(format!("{:?}", lower.time())),
            Span::raw(format!("{:?}", midpoint.time())),
            Span::raw(format!("{:?}", upper.time())),
        ];
    }

    fn y_axis_labels(&self, bounds: [f64; 2]) -> Vec<Span> {
        // Create 7 labels for our y axis, based on the y-axis bounds we computed above.
        let min = bounds[0];
        let max = bounds[1];

        let difference = max - min;
        let num_labels = 7;
        // Split difference into one chunk for each of the 7 labels
        let increment = Duration::from_micros((difference / num_labels as f64) as u64);
        let duration = Duration::from_micros(min as u64);

        (0..num_labels)
            .map(|i| Span::raw(format!("{:?}", duration.add(increment * i))))
            .collect()
    }
}

#[derive(Debug)]
enum Update {
    Result(Duration),
    Timeout,
    Unknown,
}

impl From<PingResult> for Update {
    fn from(result: PingResult) -> Self {
        match result {
            PingResult::Pong(duration, _) => Update::Result(duration),
            PingResult::Timeout(_) => Update::Timeout,
            PingResult::Unknown(_) => Update::Unknown,
        }
    }
}

#[derive(Debug)]
enum Event {
    Update(usize, Update),
    Input(KeyEvent),
    Ctrlc
}

fn start_cmd_thread(
    watch_cmd: &str,
    host_id: usize,
    watch_interval: f32,
    cmd_tx: Sender<Event>,
    kill_event: Arc<AtomicBool>,
) -> JoinHandle<Result<()>> {
    let mut words = watch_cmd.split_ascii_whitespace();
    let cmd = words
        .next()
        .expect("Must specify a command to watch")
        .to_string();
    let cmd_args = words
        .into_iter()
        .map(|w| w.to_string())
        .collect::<Vec<String>>();

    let interval = Duration::from_millis((watch_interval * 1000.0) as u64);

    // Pump cmd watches into the queue
    thread::spawn(move || -> Result<()> {
        while !kill_event.load(Ordering::Acquire) {
            let start = Instant::now();
            let mut child = Command::new(&cmd)
                .args(&cmd_args)
                .stderr(Stdio::null())
                .stdout(Stdio::null())
                .spawn()?;
            let status = child.wait()?;
            let duration = start.elapsed();
            let update = if status.success() {
                Update::Result(duration)
            } else {
                Update::Timeout
            };
            cmd_tx.send(Event::Update(host_id, update))?;
            thread::sleep(interval);
        }
        Ok(())
    })
}

fn start_ping_thread(
    host: String,
    host_id: usize,
    ping_tx: Sender<Event>,
    kill_event: Arc<AtomicBool>,
) -> JoinHandle<Result<()>> {
    // Pump ping messages into the queue
    thread::spawn(move || -> Result<()> {
        let stream = ping(host)?;
        while !kill_event.load(Ordering::Acquire) {
            ping_tx.send(Event::Update(host_id, stream.recv()?.into()))?;
        }
        Ok(())
    })
}

fn get_host_ipaddr(host: &str, force_ipv4: bool, force_ipv6: bool) -> Result<String> {
    let ipaddr: Vec<IpAddr> = match lookup_host(host) {
        Ok(ip) => ip,
        Err(_) => return Err(anyhow!("Could not resolve hostname {}", host)),
    };
    let ipaddr = if force_ipv4 {
        ipaddr
            .iter()
            .find(|ip| matches!(ip, IpAddr::V4(_)))
            .ok_or_else(|| anyhow!("Could not resolve '{}' to IPv4", host))
    } else if force_ipv6 {
        ipaddr
            .iter()
            .find(|ip| matches!(ip, IpAddr::V6(_)))
            .ok_or_else(|| anyhow!("Could not resolve '{}' to IPv6", host))
    } else {
        ipaddr
            .first()
            .ok_or_else(|| anyhow!("Could not resolve '{}' to IP", host))
    };
    Ok(ipaddr?.to_string())
}

fn main() -> Result<()> {
    let mut args = Args::from_args();
    
    #[cfg(target_os="windows")]
    {args.simple_graphics = true;}
    
    let enable_map = if args.hosts_or_commands.len() == 0 {
        print!("no hosts given, pinging the desired three hosts determined by tracert... : ");
        let hops = find_hops::get_desired_hops();
        args.hosts_or_commands.extend_from_slice(&hops);
        println!("{}, {}, {}", hops[0], hops[1], hops[2]);
        true
    } else {
        true
    };

    let mut data = vec![];

    for (idx, host_or_cmd) in args.hosts_or_commands.iter().enumerate() {
        let display = match args.cmd {
            true => host_or_cmd.to_string(),
            false => format!(
                "{} ({})",
                host_or_cmd,
                get_host_ipaddr(host_or_cmd, args.ipv4, args.ipv6)?
            ),
        };
        
        let color = if idx < HOP_COLORS.len() {
            HOP_COLORS[idx]
        } else {
            Color::Indexed(idx as u8 - HOP_COLORS.len() as u8 + 1)
        };
        data.push(PlotData::new(
            display,
            args.buffer,
            Style::default().fg(color),
            args.simple_graphics
        ));
    }

    let mut app = App::new(data, args.buffer);
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);

    let mut terminal = Terminal::new(backend)?;

    terminal.clear()?;

    let (key_tx, rx) = mpsc::channel();
    
    let ctrlc_tx = key_tx.clone();
    ctrlc::set_handler(move || {
        ctrlc_tx.send(Event::Ctrlc).unwrap();
    }).expect("Error setting Ctrl-C handler");

    let mut threads = vec![];

    let killed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    for (host_id, host_or_cmd) in args.hosts_or_commands.iter().cloned().enumerate() {
        if args.cmd {
            let cmd_thread = start_cmd_thread(
                &host_or_cmd,
                host_id,
                args.watch_interval,
                key_tx.clone(),
                std::sync::Arc::clone(&killed),
            );
            threads.push(cmd_thread);
        } else {
            threads.push(start_ping_thread(
                host_or_cmd,
                host_id,
                key_tx.clone(),
                std::sync::Arc::clone(&killed),
            ));
        }
    }

    // Pump keyboard messages into the queue
    let killed_thread = std::sync::Arc::clone(&killed);
    let key_thread = thread::spawn(move || -> Result<()> {
        while !killed_thread.load(Ordering::Acquire) {
            if event::poll(Duration::from_millis(100))? {
                if let CEvent::Key(key) = event::read()? {
                    key_tx.send(Event::Input(key))?;
                }
            }
        }
        Ok(())
    });
    threads.push(key_thread);
    
    let mut logger = log::CsvLogger::new(args.hosts_or_commands.len());
    
    let mut rolling_buffers : Vec<VecDeque<(Instant,Duration)>> = vec![VecDeque::new(); args.hosts_or_commands.len()];
    
    loop {
        match rx.recv()? {
            Event::Update(host_id, update) => {
                match update {
                    Update::Result(duration) => {
                        if enable_map {
                            rolling_buffers[host_id].push_back((Instant::now(),duration));
                        }
                        app.update(host_id, duration);
                        logger.log(host_id, duration);
                    },
                    Update::Timeout => {
                        app.update(host_id, Duration::from_secs(1));
                        logger.log(host_id, Duration::from_secs(1));
                    },
                    Update::Unknown => (),
                };
                terminal.draw(|f| {
                    // Split our
                    let mut chart_height = f.size().height 
                        - app.data.len() as u16
                        - 2; // margin
                    if enable_map { chart_height -= 4; }
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .margin(1)
                        .constraints(
                            if enable_map {
                                iter::repeat(Constraint::Length(1))
                                    .take(app.data.len())
                                    .chain(iter::once(Constraint::Length(chart_height)))
                                    .chain(iter::once(Constraint::Length(4)))
                                    .collect::<Vec<_>>()
                            } else {
                                iter::repeat(Constraint::Length(1))
                                    .take(app.data.len())
                                    .chain(iter::once(Constraint::Length(chart_height)))
                                    .collect::<Vec<_>>()
                            }
                            
                        )
                        .split(f.size());

                    let total_chunks = chunks.len();
                    
                    let n = if enable_map { 2 } else { 1 };
                    
                    let header_chunks = chunks[0..total_chunks - n].to_owned();
                    let chart_chunk = chunks[total_chunks - n].to_owned();

                    for (plot_data, chunk) in app.data.iter().zip(header_chunks) {
                        let header_layout = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints(
                                [
                                    Constraint::Percentage(20),
                                    Constraint::Percentage(20),
                                    Constraint::Percentage(20),
                                    Constraint::Percentage(20),
                                    Constraint::Percentage(20),
                                ]
                                .as_ref(),
                            )
                            .split(chunk);

                        for (area, paragraph) in
                            header_layout.into_iter().zip(plot_data.header_stats())
                        {
                            f.render_widget(paragraph, area);
                        }
                    }

                    let datasets: Vec<Dataset> = app.data.iter().map(|d| d.into()).collect();

                    let y_axis_bounds = app.y_axis_bounds();
                    let x_axis_bounds = app.x_axis_bounds();

                    let chart = Chart::new(datasets)
                        .block(Block::default().borders(Borders::NONE))
                        .x_axis(
                            Axis::default()
                                .style(Style::default().fg(Color::Gray))
                                .bounds(x_axis_bounds)
                                .labels(app.x_axis_labels(x_axis_bounds)),
                        )
                        .y_axis(
                            Axis::default()
                                .style(Style::default().fg(Color::Gray))
                                .bounds(y_axis_bounds)
                                .labels(app.y_axis_labels(y_axis_bounds)),
                        );

                    f.render_widget(chart, chart_chunk);
                    
                    if enable_map {
                        let map_chunk = chunks[total_chunks - 1].to_owned();
                        let map_box = Block::default()
                            .borders(Borders::ALL);
                        let map_inner = map_box.inner(map_chunk);
                        f.render_widget(map_box, map_chunk);
                        
                        let extra_chunk_width = args.hosts_or_commands.last().unwrap().len().max("Internet Hop 2".len()) as u16;
                        let width = map_inner.width;
                        if width <= extra_chunk_width { return; }
                        let remaining_width = width - extra_chunk_width;
                        let num_normal_chunks = args.hosts_or_commands.len() as u16;
                        let width_per_chunk = remaining_width / num_normal_chunks;
                        
                        let subchunks = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints(
                                iter::repeat(Constraint::Length(width_per_chunk))
                                .take(num_normal_chunks as usize)
                                .chain(iter::once(Constraint::Length(extra_chunk_width)))
                                .collect::<Vec<_>>()
                                .as_ref()
                            )
                            .split(map_inner);
                        
                        for (i, (host, chunk)) in iter::once("Your device")
                            .chain(args.hosts_or_commands[..args.hosts_or_commands.len()-1] .iter().map(|s| s.as_str()))
                            
                            .zip(subchunks.clone())
                            .enumerate()
                        {
                            let name = match i {
                                0 => "".to_owned(),
                                1 => "Home Gateway".to_owned(),
                                n => format!("Internet Hop {}",n-1)
                            };
                            
                            let mut line2 = chunk.clone();
                            line2.y += 1;
                            if line2.height == 0 { return; }
                            line2.height -= 1;
                            
                            f.render_widget(Block::default().title(Span::raw(name)), chunk);
                            f.render_widget(Block::default().title(Span::raw(host)), line2);
                            
                            line2.x += host.len() as u16;
                            if line2.width > host.len() as u16 {
                                line2.width -= host.len() as u16
                            } else {
                                line2.width = 0;
                            }
                            
                            let next_hop_latancy = rolling_buffers[i]
                                .iter()
                                .map(|(_, l)| l.clone())
                                .max()
                                .unwrap_or(Duration::from_secs(0));
                            
                            let latancy = if i > 0 {
                                let this_hop_latancy = rolling_buffers[i-1]
                                    .iter()
                                    .map(|(_, l)| l.clone())
                                    .max()
                                    .unwrap_or(Duration::from_secs(0));
                                
                                if this_hop_latancy > next_hop_latancy {
                                    Duration::from_secs(0)
                                } else {
                                    next_hop_latancy - this_hop_latancy
                                }
                            } else {
                                next_hop_latancy
                            };
                            
                            let color = if latancy <= Duration::from_millis(30) {
                                Color::Green
                            } else if latancy <= Duration::from_millis(60) {
                                Color::Yellow
                            } else if latancy <= Duration::from_millis(90) {
                                Color::Rgb(0xFF, 0xA4, 0x00)
                            } else {
                                Color::Red
                            };
                            
                            let mut bar = String::new();
                            for _ in 0..line2.width {
                                bar.push_str(tui::symbols::line::THICK_HORIZONTAL);
                            }
                            f.render_widget(Block::default().title(Span::styled(bar, Style::default().fg(color))), line2);
                            
                            
                            loop {
                                if let Some((recorded, _)) = rolling_buffers[i].front() {
                                    if recorded.elapsed().as_secs() > 10 {
                                        rolling_buffers[i].pop_front();
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                            
                            
                            let latancy_string = format!("{:?}",latancy);
                            
                            if line2.width >= latancy_string.len() as u16 {
                                let offset = (line2.width - latancy_string.len() as u16)/2;
                                line2.x += offset;
                                line2.width -= offset;
                                line2.y -= 1;
                                f.render_widget(Block::default().title(Span::raw(latancy_string)), line2);
                            }
                        }
                        
                        let mut extra_chunk = map_inner.clone();
                        extra_chunk.width -= remaining_width;
                        extra_chunk.x += remaining_width;
                        let mut extra_chunk2 = extra_chunk.clone();
                        extra_chunk2.y += 1;
                        extra_chunk2.height -= 1;
                        
                        let n = subchunks.len() -1;
                        let name = format!("Internet Hop {}",n-1);
                        f.render_widget(Block::default().title(Span::raw(name)), extra_chunk);
                        f.render_widget(Block::default().title(Span::raw(args.hosts_or_commands.last().unwrap())), extra_chunk2);
                    }
                })?;
            }
            Event::Input(input) => match input.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    killed.store(true, Ordering::Release);
                    break;
                }
                KeyCode::Char('c') if input.modifiers == KeyModifiers::CONTROL => {
                    killed.store(true, Ordering::Release);
                    break;
                }
                _ => {}
            },
            Event::Ctrlc => {
                killed.store(true, Ordering::Release);
                break;
            }
        }
    }

    for thread in threads {
        thread.join().unwrap()?;
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
