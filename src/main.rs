use chrono::NaiveDateTime;
use eframe;
use egui;
use egui::plot::{Line, Plot, PlotPoints, PlotUi};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

/// A single log entry (one line from the .ulg JSON)
#[derive(Debug, Deserialize, Serialize)]
struct LogEntry {
    timestamp: String,
    event: String,
    comment: Option<String>,

    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64, // Converted timestamp (UNIX-like) for plotting
}

/// Represents a start-end interval for ON or OFF
struct Interval {
    start: f64,
    end: f64,
}

/// Holds the ON/OFF intervals for a given signal
struct SignalData {
    name: String,  // e.g. "TALLY", "CHG", "DISP", "SKIP"
    y_offset: f64, // Which Y-level to draw this signal
    on_intervals: Vec<Interval>,
    off_intervals: Vec<Interval>,
    is_on: Option<f64>, // Used when we detect an "ON" without an immediate "OFF"
}

/// Main application
struct MyApp {
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    min_time: f64,
    max_time: f64,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("My Rust EGUI App - Multi-signal Timeline");

            // Show a scrollable list of the logs at the top
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .show(ui, |ui| {
                    for (i, log) in self.logs.iter().enumerate() {
                        ui.label(format!(
                            "{}: [{}] {} - {}",
                            i,
                            log.timestamp,
                            log.event,
                            log.comment.as_deref().unwrap_or("")
                        ));
                    }
                });

            ui.separator();
            ui.label("Timeline");

            // Plot the signals (both ON and OFF lines)
            Plot::new("multi_timeline_plot")
                .height(300.0)
                // Customize the Y-axis to display signal names instead of raw numbers
                .y_axis_formatter(|y, _range| match y.round() as i32 {
                    4 => "SKIP".to_string(),
                    3 => "DISP".to_string(),
                    2 => "CHG".to_string(),
                    1 => "TALLY".to_string(),
                    0 => "".to_string(), // no label for 0
                    _ => "".to_string(),
                })
                // Include some horizontal range if needed
                .include_x(self.min_time)
                .include_x(self.max_time)
                .show(ui, |plot_ui: &mut PlotUi| {
                    for signal_data in self.signals.values() {
                        draw_signal(plot_ui, signal_data);
                    }
                });
        });
    }
}

/// Draw ON intervals (in one color) and OFF intervals (in another color) for a single signal
fn draw_signal(plot_ui: &mut PlotUi, signal_data: &SignalData) {
    // Draw OFF intervals first (e.g. gray line)
    let off_line = intervals_to_line(&signal_data.off_intervals, signal_data.y_offset)
        .color(egui::Color32::GRAY)
        .name(format!("{} OFF", signal_data.name));
    plot_ui.line(off_line);

    // Draw ON intervals (e.g. purple line)
    let on_line = intervals_to_line(&signal_data.on_intervals, signal_data.y_offset)
        .color(egui::Color32::from_rgb(200, 100, 255))
        .name(format!("{} ON", signal_data.name));
    plot_ui.line(on_line);
}

/// Convert a list of intervals into a single `Line` (a series of segments).
/// Each interval is represented by two points: (start, y) and (end, y).
/// We insert NaN, NaN between intervals so they do not connect to each other.
fn intervals_to_line(intervals: &Vec<Interval>, y: f64) -> Line {
    let mut points = Vec::new();
    for iv in intervals {
        points.push([iv.start, y]);
        points.push([iv.end, y]);
        // Insert a break so lines do not connect
        points.push([f64::NAN, f64::NAN]);
    }
    Line::new(PlotPoints::from(points))
}

/// Parse timestamps into a float. This example replaces 'T' with ' ' and removes 'Z'.
/// You can adjust to fully handle RFC3339 with `DateTime::parse_from_rfc3339` if needed.
fn parse_timestamp_to_f64(ts: &str) -> f64 {
    let replaced = ts.replace('T', " ").replace('Z', "");
    if let Ok(ndt) = NaiveDateTime::parse_from_str(&replaced, "%Y-%m-%d %H:%M:%S%.3f") {
        let epoch =
            NaiveDateTime::parse_from_str("1970-01-01 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        (ndt - epoch).num_milliseconds() as f64 / 1000.0
    } else {
        0.0
    }
}

/// Return a short name for the signal based on event text.
fn get_signal_name(event: &str) -> &str {
    if event.contains("TALLY") {
        "TALLY"
    } else if event.contains("CHG") {
        "CHG"
    } else if event.contains("DISP") {
        "DISP"
    } else if event.contains("SKIP") {
        "SKIP"
    } else {
        "OTHER"
    }
}

/// Update the signal intervals based on the event.
/// TALLY=ON / OFF or PULSE(400) etc. can be handled here.
fn update_signal_data(signals: &mut HashMap<String, SignalData>, event: &str, time: f64) {
    let name = get_signal_name(event);
    if let Some(sig) = signals.get_mut(name) {
        // TALLY=ON
        if event.contains("TALLY=ON") {
            sig.is_on = Some(time);
        }
        // TALLY=OFF
        else if event.contains("TALLY=OFF") {
            if let Some(start) = sig.is_on.take() {
                sig.on_intervals.push(Interval { start, end: time });
            }
        }
        // PULSE(400): create a short ON interval from time to time+0.4
        else if event.contains("PULSE(400)") {
            sig.on_intervals.push(Interval {
                start: time,
                end: time + 0.4,
            });
        }
        // DISP(...): treat as a short ON interval (0.2s) for demonstration
        else if event.contains("DISP(") {
            sig.on_intervals.push(Interval {
                start: time,
                end: time + 0.2,
            });
        }
        // If it's "OTHER", do nothing special
    }
}

/// Once all ON intervals are collected, fill the OFF intervals for each signal
/// from the global min_time to max_time. We also merge overlapping ON intervals
/// so that OFF intervals can be computed properly.
fn finalize_on_off_intervals(signal_data: &mut SignalData, min_t: f64, max_t: f64) {
    // Merge overlapping ON intervals
    signal_data
        .on_intervals
        .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());

    let mut merged: Vec<Interval> = Vec::new();
    for iv in &signal_data.on_intervals {
        if let Some(last_iv) = merged.last_mut() {
            if iv.start <= last_iv.end {
                // Overlap: extend the end if needed
                if iv.end > last_iv.end {
                    last_iv.end = iv.end;
                }
            } else {
                merged.push(Interval {
                    start: iv.start,
                    end: iv.end,
                });
            }
        } else {
            merged.push(Interval {
                start: iv.start,
                end: iv.end,
            });
        }
    }
    signal_data.on_intervals = merged;

    // Create OFF intervals from the gaps
    let mut off = Vec::new();
    let mut current = min_t;
    for iv in &signal_data.on_intervals {
        if iv.start > current {
            off.push(Interval {
                start: current,
                end: iv.start,
            });
        }
        current = iv.end;
    }
    if current < max_t {
        off.push(Interval {
            start: current,
            end: max_t,
        });
    }
    signal_data.off_intervals = off;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1) Read the .ulg (JSON) file
    let path = "example.ulg"; // Adjust if needed
    let data = fs::read_to_string(path)?;
    let mut logs: Vec<LogEntry> = serde_json::from_str(&data)?;

    // 2) Convert timestamps to f64 and sort by time
    for log in &mut logs {
        log.timestamp_num = parse_timestamp_to_f64(&log.timestamp);
    }
    logs.sort_by(|a, b| a.timestamp_num.partial_cmp(&b.timestamp_num).unwrap());

    // Determine global min_time and max_time
    let min_time = logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
    let max_time = logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

    // 3) Prepare signals (assign each signal a different y_offset)
    // For example, TALLY=1, CHG=2, DISP=3, SKIP=4
    let mut signals = HashMap::new();
    signals.insert(
        "TALLY".to_string(),
        SignalData {
            name: "TALLY".to_string(),
            y_offset: 1.0,
            on_intervals: vec![],
            off_intervals: vec![],
            is_on: None,
        },
    );
    signals.insert(
        "CHG".to_string(),
        SignalData {
            name: "CHG".to_string(),
            y_offset: 2.0,
            on_intervals: vec![],
            off_intervals: vec![],
            is_on: None,
        },
    );
    signals.insert(
        "DISP".to_string(),
        SignalData {
            name: "DISP".to_string(),
            y_offset: 3.0,
            on_intervals: vec![],
            off_intervals: vec![],
            is_on: None,
        },
    );
    signals.insert(
        "SKIP".to_string(),
        SignalData {
            name: "SKIP".to_string(),
            y_offset: 4.0,
            on_intervals: vec![],
            off_intervals: vec![],
            is_on: None,
        },
    );

    // 4) Build ON intervals from the log events
    for log in &logs {
        update_signal_data(&mut signals, &log.event, log.timestamp_num);
    }
    // If a signal was left ON at the end, you could close it here if desired.

    // 5) For each signal, generate OFF intervals from the gaps
    for sig in signals.values_mut() {
        finalize_on_off_intervals(sig, min_time, max_time);
    }

    // 6) Create and run the application
    let app = MyApp {
        logs,
        signals,
        min_time,
        max_time,
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "My Rust EGUI App - Multi-signal Timeline",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )?;

    Ok(())
}
