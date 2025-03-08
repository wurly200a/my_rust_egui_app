// main.rs

use chrono::{Duration, NaiveDateTime, TimeZone, Utc};
use eframe;
use egui;
use egui::plot::{Legend, Line, Plot, PlotPoints, PlotUi};
use egui::Color32;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::ops::RangeInclusive;

/// A single log entry from the .ulg (JSON).
#[derive(Debug, Deserialize, Serialize)]
struct LogEntry {
    timestamp: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    // Add a group field for grouping
    #[serde(default)]
    group: Option<String>,
    value: Value,
    comment: Option<String>,

    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64,
}

/// Represents an ON interval.
struct Interval {
    start: f64,
    end: f64,
}

/// Holds ON intervals for one signal.
struct SignalData {
    name: String,
    y_offset: f64, // Baseline (OFF) position
    on_intervals: Vec<Interval>,
    is_on: Option<f64>, // For ONOFF: track start time
    visible: bool,      // Whether this signal is visible
}

/// Holds a group of signals.
struct GroupData {
    name: String,
    signals: Vec<String>, // signal names
}

/// Main application
struct MyApp {
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    offset_to_name: HashMap<i32, String>,
    min_time: f64,
    max_time: f64,

    // Group management
    groups: HashMap<String, GroupData>,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Left side panel for group/signal checkboxes
        egui::SidePanel::left("group_panel")
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Groups");

                // Show each group with a checkbox and indent for signals
                for group in self.groups.values_mut() {
                    // Count how many signals are visible vs. total
                    let visible_count = group
                        .signals
                        .iter()
                        .filter(|s| self.signals[*s].visible)
                        .count();

                    // If at least one signal is visible => group checkbox is "checked"
                    let mut group_check = visible_count > 0;

                    // Show the group checkbox
                    let group_response = ui.checkbox(&mut group_check, &group.name);

                    // If group checkbox was toggled
                    if group_response.changed() {
                        // Turn all signals in this group on/off
                        for s in &group.signals {
                            if let Some(sig) = self.signals.get_mut(s) {
                                sig.visible = group_check;
                            }
                        }
                    }

                    // Show signals in an indented area
                    ui.indent(format!("group_indent_{}", group.name), |ui| {
                        for s in &group.signals {
                            if let Some(sig) = self.signals.get_mut(s) {
                                let mut check = sig.visible;
                                if ui.checkbox(&mut check, &sig.name).changed() {
                                    sig.visible = check;
                                }
                            }
                        }
                    });

                    ui.separator();
                }
            });

        // Central panel for waveform plot and log display
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("My Rust EGUI App - Single-Step ON/OFF Waveform");

            // Simple log display
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
            ui.label("Timeline (Digital Waveform)");

            // Clone for move in plot
            let offset_to_name = self.offset_to_name.clone();

            // X-axis date-time formatter
            let x_axis_formatter = |x: f64, _range: &RangeInclusive<f64>| {
                // Use Unix epoch as base
                let dt =
                    Utc.timestamp(0, 0).naive_utc() + Duration::milliseconds((x * 1000.0) as i64);
                dt.format("%H:%M:%S%.3f").to_string()
            };

            // Expand the plot widget
            Plot::new("digital_wave_plot")
                .min_size(ui.available_size())
                .include_x(self.min_time)
                .include_x(self.max_time)
                .x_axis_formatter(x_axis_formatter)
                .y_axis_formatter(move |y, _range| {
                    let y_int = y.round() as i32;
                    offset_to_name
                        .get(&y_int)
                        .cloned()
                        .unwrap_or_else(|| "".to_string())
                })
                .legend(Legend::default())
                .show(ui, |plot_ui: &mut PlotUi| {
                    // Color palette for signals
                    let color_palette = [
                        Color32::RED,
                        Color32::GREEN,
                        Color32::BLUE,
                        Color32::YELLOW,
                        Color32::LIGHT_BLUE,
                        Color32::LIGHT_GREEN,
                        Color32::WHITE,
                        Color32::GOLD,
                    ];

                    // Draw waveforms only for visible signals
                    let mut draw_index = 0;
                    for signal_data in self.signals.values() {
                        if !signal_data.visible {
                            continue;
                        }
                        let wave_line = build_digital_wave(
                            &signal_data.on_intervals,
                            self.min_time,
                            self.max_time,
                            signal_data.y_offset,
                        );

                        let color = color_palette[draw_index % color_palette.len()];
                        draw_index += 1;

                        plot_ui.line(wave_line.color(color).width(2.0).name(&signal_data.name));
                    }
                });
        });
    }
}

/// Build a single "digital wave" line from on_intervals.
/// OFF = y_offset, ON = y_offset + 1
fn build_digital_wave(on_intervals: &Vec<Interval>, min_t: f64, max_t: f64, offset: f64) -> Line {
    let mut points = Vec::new();
    let mut current_x = min_t;

    // Start with OFF
    points.push([current_x, offset]);

    for iv in on_intervals {
        if iv.start > current_x {
            points.push([iv.start, offset]);
        }
        // Step up to ON
        points.push([iv.start, offset + 1.0]);
        // Keep ON
        points.push([iv.end, offset + 1.0]);
        // Step down to OFF
        points.push([iv.end, offset]);
        current_x = iv.end;
    }

    if current_x < max_t {
        points.push([max_t, offset]);
    }

    Line::new(PlotPoints::from(points))
}

/// Parse timestamp string (ISO8601) to f64 (Unix epoch in seconds)
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

/// Update ON intervals from the log (for ONOFF/PULSE/ARROW etc.)
fn update_signal_data(signals: &mut HashMap<String, SignalData>, log: &LogEntry) {
    let signal_name = &log.name;
    let time = log.timestamp_num;

    match log.kind.as_str() {
        "ONOFF" => {
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
            if let Some(_ms) = log.value.as_f64() {
                if let Some(sig) = signals.get_mut(signal_name) {
                    sig.on_intervals.push(Interval {
                        start: time,
                        end: time + 0.001,
                    });
                }
            }
        }
        "ARROW" => {
            if let Some(sig) = signals.get_mut(signal_name) {
                sig.on_intervals.push(Interval {
                    start: time,
                    end: time + 0.2,
                });
            }
        }
        _ => {
            if let Some(sig) = signals.get_mut(signal_name) {
                sig.on_intervals.push(Interval {
                    start: time,
                    end: time + 0.2,
                });
            }
        }
    }
}

/// Merge overlapping intervals
fn merge_on_intervals(sig: &mut SignalData) {
    sig.on_intervals
        .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let mut merged: Vec<Interval> = Vec::new();

    for iv in &sig.on_intervals {
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
    sig.on_intervals = merged;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "example.ulg";
    let data = fs::read_to_string(path)?;
    let mut logs: Vec<LogEntry> = serde_json::from_str(&data)?;

    // Convert timestamp to numeric
    for log in &mut logs {
        log.timestamp_num = parse_timestamp_to_f64(&log.timestamp);
    }
    logs.sort_by(|a, b| a.timestamp_num.partial_cmp(&b.timestamp_num).unwrap());

    let min_time = logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
    let max_time = logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

    // Collect unique signal names
    let mut unique_names = BTreeSet::new();
    for log in &logs {
        unique_names.insert(log.name.clone());
    }

    let mut signals = HashMap::new();
    let mut offset_to_name = HashMap::new();

    // Prepare groups
    let mut groups = HashMap::<String, GroupData>::new();

    // Create signals with offset from top to bottom
    let mut unique_names: Vec<String> = unique_names.into_iter().collect();
    let n = unique_names.len();
    for (i, name) in unique_names.iter().enumerate() {
        let y_offset = ((n - i) * 2 - 1) as f64;

        signals.insert(
            name.clone(),
            SignalData {
                name: name.clone(),
                y_offset,
                on_intervals: vec![],
                is_on: None,
                visible: true, // default to visible
            },
        );
        offset_to_name.insert(y_offset as i32, name.clone());
    }

    // For each log entry, record the signal -> group relation
    let mut signal_to_group = HashMap::new();
    for log in &logs {
        if let Some(grp) = &log.group {
            if !grp.is_empty() {
                // Create group if not exists
                if !groups.contains_key(grp) {
                    groups.insert(
                        grp.clone(),
                        GroupData {
                            name: grp.clone(),
                            signals: Vec::new(),
                        },
                    );
                }
                // Assign this signal to that group (once per signal)
                if !signal_to_group.contains_key(&log.name) {
                    signal_to_group.insert(log.name.clone(), grp.clone());
                }
            }
        }
    }

    // Fill group -> signals from signal_to_group
    for (signal_name, group_name) in signal_to_group {
        if let Some(g) = groups.get_mut(&group_name) {
            // Avoid duplication
            if !g.signals.contains(&signal_name) {
                g.signals.push(signal_name);
            }
        }
    }

    // Sort the signal names inside each group
    for g in groups.values_mut() {
        g.signals.sort();
    }

    // Update on_intervals based on logs
    for log in &logs {
        update_signal_data(&mut signals, log);
    }

    // Merge intervals for each signal
    for sig in signals.values_mut() {
        merge_on_intervals(sig);
    }

    let app = MyApp {
        logs,
        signals,
        offset_to_name,
        min_time,
        max_time,
        groups,
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "My Rust EGUI App - Single-Step ON/OFF Waveform",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )?;

    Ok(())
}
