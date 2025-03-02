use chrono::{DateTime, Utc};
use eframe;
use egui;
use egui::plot::{Plot, PlotPoints, PlotUi, Points};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Deserialize, Serialize)]
struct LogEntry {
    timestamp: String,
    event: String,
    comment: Option<String>,

    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64,
}

struct MyApp {
    logs: Vec<LogEntry>,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("ULG Log Viewer (Rust + egui)");

            egui::ScrollArea::vertical().show(ui, |ui| {
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

            Plot::new("timeline_plot")
                .height(200.0)
                .show(ui, |plot_ui: &mut PlotUi| {
                    let points: PlotPoints = self
                        .logs
                        .iter()
                        .map(|log| [log.timestamp_num, 0.0])
                        .collect();

                    let points = Points::new(points).radius(3.0).name("Events");
                    plot_ui.points(points);
                });
        });
    }
}

fn parse_timestamp_to_f64(ts: &str) -> f64 {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        dt.timestamp_millis() as f64
    } else {
        0.0
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "example.ulg";
    let data = fs::read_to_string(path)?;
    let mut logs: Vec<LogEntry> = serde_json::from_str(&data)?;

    for log in &mut logs {
        log.timestamp_num = parse_timestamp_to_f64(&log.timestamp);
    }

    for log in &logs {
        println!("{} => {}", log.timestamp, log.timestamp_num);
    }

    let app = MyApp { logs };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "My Rust EGUI App",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )?;

    Ok(())
}
