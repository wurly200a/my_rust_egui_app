use chrono::NaiveDateTime;
use eframe;
use egui;
use egui::plot::{Line, Plot, PlotPoints, PlotUi};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;

/// A single log entry from the .ulg (JSON) file.
/// The "type" field is renamed to `kind` because "type" is a Rust keyword.
#[derive(Debug, Deserialize, Serialize)]
struct LogEntry {
    timestamp: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    value: Value,
    comment: Option<String>,

    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64, // Converted timestamp for plotting (in seconds)
}

/// Represents a start–end interval for an ON or OFF period.
struct Interval {
    start: f64,
    end: f64,
}

/// Holds ON/OFF intervals for one signal.
struct SignalData {
    name: String,  // e.g. "TALLY", "CHG", etc.
    y_offset: f64, // Which Y-level to draw this signal on the plot.
    on_intervals: Vec<Interval>,
    off_intervals: Vec<Interval>,
    is_on: Option<f64>, // Used to track an ON event until an OFF event is seen.
}

/// Main application.
struct MyApp {
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    /// Mapping from y_offset (as integer) to signal names for the y-axis labels.
    offset_to_name: HashMap<i32, String>,
    min_time: f64,
    max_time: f64,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("My Rust EGUI App - Multi-signal Timeline (Generic)");

            // Display raw logs at the top.
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .show(ui, |ui| {
                    for (i, log) in self.logs.iter().enumerate() {
                        ui.label(format!(
                            "{}: [{}] {} - {} - {}",
                            i,
                            log.timestamp,
                            log.name,
                            log.kind,
                            log.comment.as_deref().unwrap_or("")
                        ));
                    }
                });

            ui.separator();
            ui.label("Timeline");

            // Clone the offset mapping so it can be moved into the closure.
            let offset_to_name = self.offset_to_name.clone();
            Plot::new("multi_timeline_plot")
                .height(300.0)
                .y_axis_formatter(move |y, _range| {
                    let y_int = y.round() as i32;
                    offset_to_name
                        .get(&y_int)
                        .cloned()
                        .unwrap_or_else(|| "".to_string())
                })
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

/// Draws the ON intervals (in one color) and the OFF intervals (in another color) for one signal.
fn draw_signal(plot_ui: &mut PlotUi, signal_data: &SignalData) {
    // Draw OFF intervals first (gray).
    let off_line = intervals_to_line(&signal_data.off_intervals, signal_data.y_offset)
        .color(egui::Color32::GRAY)
        .name(format!("{} OFF", signal_data.name));
    plot_ui.line(off_line);

    // Draw ON intervals (purple).
    let on_line = intervals_to_line(&signal_data.on_intervals, signal_data.y_offset)
        .color(egui::Color32::from_rgb(200, 100, 255))
        .name(format!("{} ON", signal_data.name));
    plot_ui.line(on_line);
}

/// Converts a list of intervals into a single Line (a series of segments).
/// Inserts gaps (NaN values) so that separate intervals are not connected.
fn intervals_to_line(intervals: &Vec<Interval>, y: f64) -> Line {
    let mut points = Vec::new();
    for iv in intervals {
        points.push([iv.start, y]);
        points.push([iv.end, y]);
        points.push([f64::NAN, f64::NAN]);
    }
    Line::new(PlotPoints::from(points))
}

/// Parses a timestamp string into a float (seconds).
/// Adjust the format if your dates differ.
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

/// Updates the signal intervals based on the log entry.
/// - For "ONOFF" type, the value is a string ("ON"/"OFF").
/// - For "PULSE" type, the value is a number (milliseconds).
/// - For "ARROW" type, we treat it as a short pulse (0.2 seconds).
fn update_signal_data(signals: &mut HashMap<String, SignalData>, log: &LogEntry) {
    let signal_name = &log.name;
    let time = log.timestamp_num;
    match log.kind.as_str() {
        "ONOFF" => {
            // Expect value to be a string.
            if let Some(val) = log.value.as_str() {
                if val == "ON" {
                    if let Some(sig) = signals.get_mut(signal_name) {
                        sig.is_on = Some(time);
                    }
                } else if val == "OFF" {
                    if let Some(sig) = signals.get_mut(signal_name) {
                        if let Some(start) = sig.is_on.take() {
                            sig.on_intervals.push(Interval { start, end: time });
                        }
                    }
                }
            }
        }
        "PULSE" => {
            // Expect value to be a number (milliseconds).
            if let Some(duration_ms) = log.value.as_f64() {
                if let Some(sig) = signals.get_mut(signal_name) {
                    sig.on_intervals.push(Interval {
                        start: time,
                        end: time + duration_ms / 1000.0,
                    });
                }
            }
        }
        "ARROW" => {
            // Treat as a short pulse (0.2 seconds).
            if let Some(sig) = signals.get_mut(signal_name) {
                sig.on_intervals.push(Interval {
                    start: time,
                    end: time + 0.2,
                });
            }
        }
        _ => {
            // Default: treat as a short pulse.
            if let Some(sig) = signals.get_mut(signal_name) {
                sig.on_intervals.push(Interval {
                    start: time,
                    end: time + 0.2,
                });
            }
        }
    }
}

/// Merges overlapping ON intervals for a signal and derives OFF intervals as gaps.
fn finalize_on_off_intervals(signal_data: &mut SignalData, min_t: f64, max_t: f64) {
    // 1) Sort and merge overlapping ON intervals.
    signal_data
        .on_intervals
        .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());

    let mut merged: Vec<Interval> = Vec::new();
    for iv in &signal_data.on_intervals {
        if let Some(last_iv) = merged.last_mut() {
            if iv.start <= last_iv.end {
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

    // 2) Derive OFF intervals from the gaps.
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
    // 1) Read the .ulg (JSON) file.
    let path = "example.ulg"; // Adjust as needed.
    let data = fs::read_to_string(path)?;
    let mut logs: Vec<LogEntry> = serde_json::from_str(&data)?;

    // 2) Convert timestamps to f64 and sort logs chronologically.
    for log in &mut logs {
        log.timestamp_num = parse_timestamp_to_f64(&log.timestamp);
    }
    logs.sort_by(|a, b| a.timestamp_num.partial_cmp(&b.timestamp_num).unwrap());

    let min_time = logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
    let max_time = logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

    // 3) Gather all unique signal names from the logs.
    let mut unique_names = BTreeSet::new();
    for log in &logs {
        unique_names.insert(log.name.clone());
    }

    // 4) Dynamically assign each signal a Y offset and build the axis label mapping.
    let mut signals = HashMap::new();
    let mut offset_to_name = HashMap::new();
    let mut i = 1;
    for name in unique_names {
        signals.insert(
            name.clone(),
            SignalData {
                name: name.clone(),
                y_offset: i as f64,
                on_intervals: vec![],
                off_intervals: vec![],
                is_on: None,
            },
        );
        offset_to_name.insert(i, name);
        i += 1;
    }

    // 5) Build ON intervals from the logs.
    for log in &logs {
        update_signal_data(&mut signals, log);
    }
    // (If a signal remains ON at the end, you might finalize it here.)

    // 6) Compute OFF intervals for each signal.
    for sig in signals.values_mut() {
        finalize_on_off_intervals(sig, min_time, max_time);
    }

    // 7) Run the application.
    let app = MyApp {
        logs,
        signals,
        offset_to_name,
        min_time,
        max_time,
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "My Rust EGUI App - Multi-signal Timeline (Generic)",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )?;

    Ok(())
}
