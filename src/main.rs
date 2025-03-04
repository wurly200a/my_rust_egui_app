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
}

/// Main application
struct MyApp {
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    offset_to_name: HashMap<i32, String>,
    min_time: f64,
    max_time: f64,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("My Rust EGUI App - Single-Step ON/OFF Waveform");

            // 簡易ログ表示
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

            // Y軸ラベル用（シグナル名）をクローンしてmove
            let offset_to_name = self.offset_to_name.clone();

            // X軸日時フォーマッタ
            let x_axis_formatter = |x: f64, _range: &RangeInclusive<f64>| {
                // Utc.timestamp(0, 0) を使って 1970-01-01 00:00:00 UTC を基点とする
                let dt =
                    Utc.timestamp(0, 0).naive_utc() + Duration::milliseconds((x * 1000.0) as i64);
                dt.format("%H:%M:%S%.3f").to_string()
            };

            // Plotウィジェットのサイズをウィンドウ全体に広げるため、min_sizeを利用
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
                    // カラーパレットによるシグナルごとの色分け
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

                    for (i, signal_data) in self.signals.values().enumerate() {
                        let wave_line = build_digital_wave(
                            &signal_data.on_intervals,
                            self.min_time,
                            self.max_time,
                            signal_data.y_offset,
                        );

                        let color = color_palette[i % color_palette.len()];

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

    // 開始はOFF状態
    points.push([current_x, offset]);

    for iv in on_intervals {
        if iv.start > current_x {
            points.push([iv.start, offset]);
        }
        points.push([iv.start, offset + 1.0]); // Step up
        points.push([iv.end, offset + 1.0]); // ON状態維持
        points.push([iv.end, offset]); // Step down
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
            if let Some(ms) = log.value.as_f64() {
                if let Some(sig) = signals.get_mut(signal_name) {
                    sig.on_intervals.push(Interval {
                        start: time,
                        end: time + ms / 1000.0,
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

    for log in &mut logs {
        log.timestamp_num = parse_timestamp_to_f64(&log.timestamp);
    }
    logs.sort_by(|a, b| a.timestamp_num.partial_cmp(&b.timestamp_num).unwrap());

    let min_time = logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
    let max_time = logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

    let mut unique_names = BTreeSet::new();
    for log in &logs {
        unique_names.insert(log.name.clone());
    }

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
                is_on: None,
            },
        );
        offset_to_name.insert(i, name);
        i += 2;
    }

    for log in &logs {
        update_signal_data(&mut signals, log);
    }

    for sig in signals.values_mut() {
        merge_on_intervals(sig);
    }

    let app = MyApp {
        logs,
        signals,
        offset_to_name,
        min_time,
        max_time,
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "My Rust EGUI App - Single-Step ON/OFF Waveform",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )?;

    Ok(())
}
