#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use chrono::{Duration, TimeZone, Utc};
use eframe;
use egui;
use egui::Color32;
use egui_plot::{Legend, Line, PlotPoints, PlotUi};
#[cfg(not(target_arch = "wasm32"))]
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::ops::RangeInclusive;
use std::process::Command;

// ユーザー設定
#[derive(Debug, Serialize, Deserialize, Clone)]
struct ConversionScriptSetting {
    name: String,
    script_path: String,
    // 例: [".log", ".txt"]
    extensions: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct UserSettings {
    python_path: String,
    conversion_scripts: Vec<ConversionScriptSetting>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            python_path: "python3".to_string(),
            conversion_scripts: vec![ConversionScriptSetting {
                name: "Default Conversion".to_string(),
                script_path: "scripts/convert.py".to_string(),
                extensions: vec![".log".to_string(), ".txt".to_string()],
            }],
        }
    }
}

// ログのエントリとデータファイルの構造体
#[derive(Debug, Deserialize, Serialize)]
struct LogEntry {
    timestamp: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    #[serde(default)]
    group: Option<String>,
    value: serde_json::Value,
    comment: Option<String>,

    // 内部処理用
    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct DataFile {
    logs: Vec<LogEntry>,
    default_visibility: Option<Vec<VisibilityEntry>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct VisibilityEntry {
    group: String,
    name: String,
    visible: bool,
}

// タイムチャートの描画用データ
struct Interval {
    start: f64,
    end: f64,
}

struct SignalData {
    name: String,
    on_intervals: Vec<Interval>,
    is_on: Option<f64>,
    visible: bool,
    color: Color32,
}

struct GroupData {
    name: String,
    signals: Vec<String>,
}

#[derive(Clone)]
struct ConversionResult {
    command: String,
    stdout: String,
    stderr: String,
    ok: bool,
    json_file: Option<String>,
}

// 各ファイルごとの状態をまとめる構造体
struct FileData {
    file_name: String,
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    groups: HashMap<String, GroupData>,
    visibility_defaults: HashMap<(String, String), bool>,
    min_time: f64,
    max_time: f64,
}

impl FileData {
    /// 各ファイルのログやシグナル、グループなどを再計算する
    fn recalc(&mut self) {
        // min/max time
        self.min_time = self.logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
        self.max_time = self.logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

        // シグナル名のユニーク化
        let mut unique_names = BTreeSet::new();
        for log in &self.logs {
            unique_names.insert(log.name.clone());
        }
        let unique_names: Vec<String> = unique_names.into_iter().collect();
        self.signals.clear();
        for name in &unique_names {
            self.signals.insert(
                name.clone(),
                SignalData {
                    name: name.clone(),
                    on_intervals: vec![],
                    is_on: None,
                    visible: false,
                    color: Color32::WHITE, // 色は描画時にまとめて決めてもよい
                },
            );
        }

        // グループ作成
        self.groups.clear();
        let mut signal_to_group = HashMap::new();
        for log in &self.logs {
            if let Some(grp) = &log.group {
                if !grp.is_empty() {
                    self.groups.entry(grp.clone()).or_insert_with(|| GroupData {
                        name: grp.clone(),
                        signals: Vec::new(),
                    });
                    if !signal_to_group.contains_key(&log.name) {
                        signal_to_group.insert(log.name.clone(), grp.clone());
                    }
                }
            }
        }
        // グループにシグナルを紐づける
        for (signal_name, group_name) in &signal_to_group {
            if let Some(g) = self.groups.get_mut(group_name) {
                if !g.signals.contains(signal_name) {
                    g.signals.push(signal_name.clone());
                }
            }
        }
        for g in self.groups.values_mut() {
            g.signals.sort();
        }

        // デフォルト可視性を設定
        for (name, sig) in self.signals.iter_mut() {
            let default = if let Some(group) = signal_to_group.get(name) {
                self.visibility_defaults
                    .get(&(group.clone(), name.clone()))
                    .copied()
                    .unwrap_or(false)
            } else {
                false
            };
            sig.visible = default;
        }

        // ログを走査し on_intervals を構築
        for log in &self.logs {
            update_signal_data(&mut self.signals, log);
        }
        // interval をマージ
        for sig in self.signals.values_mut() {
            merge_on_intervals(sig);
        }
    }

    /// JSON の DataFile から FileData を生成する
    fn from_data_file(data_file: DataFile, file_path: &str) -> Self {
        let mut logs = data_file.logs;
        for log in &mut logs {
            log.timestamp_num = parse_timestamp_to_f64(&log.timestamp);
        }
        logs.sort_by(|a, b| a.timestamp_num.partial_cmp(&b.timestamp_num).unwrap());

        let mut visibility_defaults = HashMap::new();
        if let Some(defaults) = data_file.default_visibility {
            for entry in defaults {
                visibility_defaults.insert((entry.group, entry.name), entry.visible);
            }
        }

        let file_name = std::path::Path::new(file_path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let mut file_data = Self {
            file_name,
            logs,
            signals: HashMap::new(),
            groups: HashMap::new(),
            visibility_defaults,
            min_time: 0.0,
            max_time: 10.0,
        };
        file_data.recalc();
        file_data
    }
}

// ユーティリティ関数
fn parse_timestamp_to_f64(ts: &str) -> f64 {
    let replaced = ts.replace('T', " ").replace('Z', "");
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(&replaced, "%Y-%m-%d %H:%M:%S%.3f") {
        let epoch =
            chrono::NaiveDateTime::parse_from_str("1970-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        (ndt - epoch).num_milliseconds() as f64 / 1000.0
    } else {
        0.0
    }
}

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

// メインアプリケーション
struct MyApp {
    open_files: Vec<FileData>,
    conversion_result: Option<ConversionResult>,
    error_dialog_message: Option<String>,
    user_settings: UserSettings,
    settings_open: bool,
    pending_import_file: Option<String>,
    pending_script_candidates: Option<Vec<ConversionScriptSetting>>,
}

impl MyApp {
    fn new() -> Self {
        let user_settings = Self::load_settings().unwrap_or_default();
        Self {
            open_files: Vec::new(),
            conversion_result: None,
            error_dialog_message: None,
            user_settings,
            settings_open: false,
            pending_import_file: None,
            pending_script_candidates: None,
        }
    }

    fn load_settings() -> Result<UserSettings, Box<dyn std::error::Error>> {
        let settings_file = "user_settings.json";
        if let Ok(content) = fs::read_to_string(settings_file) {
            let settings: UserSettings = serde_json::from_str(&content)?;
            Ok(settings)
        } else {
            Ok(UserSettings::default())
        }
    }

    fn show_error_dialog(&mut self, message: &str) {
        eprintln!("{}", message);
        self.error_dialog_message = Some(message.to_owned());
    }

    fn execute_conversion(&mut self, file_path: &str, script: ConversionScriptSetting) {
        let command_str = format!(
            "{} {} {}",
            self.user_settings.python_path, script.script_path, file_path
        );
        let output = Command::new(&self.user_settings.python_path)
            .arg(&script.script_path)
            .arg(file_path)
            .output();
        let (stdout, stderr, ok, json_file) = match output {
            Ok(o) => {
                let ok = o.status.success();
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let json_file = if ok {
                    Some(
                        std::path::Path::new(file_path)
                            .with_extension("json")
                            .to_string_lossy()
                            .to_string(),
                    )
                } else {
                    None
                };
                (stdout, stderr, ok, json_file)
            }
            Err(e) => {
                self.show_error_dialog(&format!("Failed to execute the conversion script: {}", e));
                ("".to_string(), "".to_string(), false, None)
            }
        };
        self.conversion_result = Some(ConversionResult {
            command: command_str,
            stdout,
            stderr,
            ok,
            json_file,
        });
    }

    /// デジタル波形を生成する
    fn build_digital_wave(on_intervals: &[Interval], min_t: f64, max_t: f64, offset: f64) -> Line {
        let mut points = Vec::new();
        let mut current_x = min_t;
        points.push([current_x, offset]);
        for iv in on_intervals {
            if iv.start > current_x {
                points.push([iv.start, offset]);
            }
            points.push([iv.start, offset + 1.0]);
            points.push([iv.end, offset + 1.0]);
            points.push([iv.end, offset]);
            current_x = iv.end;
        }
        if current_x < max_t {
            points.push([max_t, offset]);
        }
        Line::new(PlotPoints::from(points))
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());

        // エラーダイアログ
        if let Some(msg) = self.error_dialog_message.clone() {
            egui::Window::new("Error")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(msg);
                    if ui.button("OK").clicked() {
                        self.error_dialog_message = None;
                    }
                });
        }

        // 変換結果ウィンドウ
        if let Some(result) = self.conversion_result.clone() {
            egui::Window::new("Conversion Result")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Command: {}", result.command));
                    ui.separator();
                    ui.label("Standard Output:");
                    egui::ScrollArea::vertical()
                        .id_salt("conversion_stdout_scroll")
                        .max_height(100.0)
                        .show(ui, |ui| {
                            ui.monospace(&result.stdout);
                        });
                    ui.separator();
                    ui.label("Error Output:");
                    egui::ScrollArea::vertical()
                        .id_salt("conversion_stderr_scroll")
                        .max_height(100.0)
                        .show(ui, |ui| {
                            ui.monospace(&result.stderr);
                        });
                    ui.separator();
                    ui.label(format!("Status: {}", if result.ok { "OK" } else { "NG" }));
                    if ui.button("OK").clicked() {
                        if result.ok {
                            if let Some(json_path) = &result.json_file {
                                match fs::read_to_string(json_path) {
                                    Ok(data) => match serde_json::from_str::<DataFile>(&data) {
                                        Ok(data_file) => {
                                            let file_data =
                                                FileData::from_data_file(data_file, json_path);
                                            self.open_files.push(file_data);
                                        }
                                        Err(_) => {
                                            self.show_error_dialog(
                                                "Failed to parse JSON data as DataFile.",
                                            );
                                        }
                                    },
                                    Err(e) => {
                                        self.show_error_dialog(&format!("File read error: {}", e));
                                    }
                                }
                            }
                        }
                        self.conversion_result = None;
                    }
                });
        }

        // pending conversion script 選択ウィンドウ
        if let (Some(file), Some(candidates)) = (
            self.pending_import_file.clone(),
            self.pending_script_candidates.clone(),
        ) {
            egui::Window::new("Select Conversion Script")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(
                        "複数の変換スクリプトが設定されています。実行するものを選択してください:",
                    );
                    for script in candidates.iter() {
                        if ui.button(&script.name).clicked() {
                            self.execute_conversion(&file, script.clone());
                            self.pending_import_file = None;
                            self.pending_script_candidates = None;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending_import_file = None;
                        self.pending_script_candidates = None;
                    }
                });
        }

        // Settings ウィンドウ
        if self.settings_open {
            let settings_open = &mut self.settings_open;
            let user_settings = &mut self.user_settings;
            egui::Window::new("Settings")
                .open(settings_open)
                .show(ctx, |ui| {
                    ui.label("Python3 Path:");
                    ui.text_edit_singleline(&mut user_settings.python_path);
                    ui.separator();
                    ui.label("Conversion Scripts:");
                    let mut remove_indices = Vec::new();
                    for (i, script) in user_settings.conversion_scripts.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut script.name);
                            ui.label("Script Path:");
                            ui.text_edit_singleline(&mut script.script_path);
                            ui.label("Extensions (comma separated):");
                            let mut ext_str = script.extensions.join(", ");
                            if ui.text_edit_singleline(&mut ext_str).changed() {
                                script.extensions = ext_str
                                    .split(',')
                                    .map(|s| s.trim().to_lowercase())
                                    .filter(|s| !s.is_empty())
                                    .map(|s| {
                                        if s.starts_with('.') {
                                            s
                                        } else {
                                            format!(".{}", s)
                                        }
                                    })
                                    .collect();
                            }
                            if ui.button("-").clicked() {
                                remove_indices.push(i);
                            }
                        });
                    }
                    for &i in remove_indices.iter().rev() {
                        user_settings.conversion_scripts.remove(i);
                    }
                    if ui.button("Add Script").clicked() {
                        user_settings
                            .conversion_scripts
                            .push(ConversionScriptSetting {
                                name: "New Script".to_string(),
                                script_path: "".to_string(),
                                extensions: vec![],
                            });
                    }
                    let mut save_error: Option<String> = None;
                    if ui.button("Save Settings").clicked() {
                        match serde_json::to_string_pretty(&*user_settings) {
                            Ok(content) => {
                                if let Err(e) = fs::write("user_settings.json", content) {
                                    save_error = Some(format!("Failed to save settings: {}", e));
                                }
                            }
                            Err(e) => {
                                save_error = Some(format!("Failed to serialize settings: {}", e));
                            }
                        }
                    }
                    if let Some(err) = save_error {
                        self.error_dialog_message = Some(err);
                    }
                });
        }

        // メニューバー
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open").clicked() {
                        ui.close_menu();
                        if let Some(path) = FileDialog::new().pick_file() {
                            let path_str = path.to_string_lossy().to_string();
                            if path_str.to_lowercase().ends_with(".json") {
                                match fs::read_to_string(&path_str) {
                                    Ok(data) => match serde_json::from_str::<DataFile>(&data) {
                                        Ok(data_file) => {
                                            let file_data =
                                                FileData::from_data_file(data_file, &path_str);
                                            self.open_files.push(file_data);
                                        }
                                        Err(_) => {
                                            self.show_error_dialog(
                                                "Failed to parse JSON data as DataFile.",
                                            );
                                        }
                                    },
                                    Err(e) => {
                                        self.show_error_dialog(&format!("File read error: {}", e));
                                    }
                                }
                            } else {
                                self.show_error_dialog("Open only supports .json files.");
                            }
                        }
                    }

                    if ui.button("Import").clicked() {
                        ui.close_menu();
                        if let Some(path) = FileDialog::new().pick_file() {
                            let path_str = path.to_string_lossy().to_string();
                            if path_str.to_lowercase().ends_with(".json") {
                                match fs::read_to_string(&path_str) {
                                    Ok(data) => match serde_json::from_str::<DataFile>(&data) {
                                        Ok(data_file) => {
                                            let file_data =
                                                FileData::from_data_file(data_file, &path_str);
                                            self.open_files.push(file_data);
                                        }
                                        Err(_) => {
                                            self.show_error_dialog(
                                                "Failed to parse JSON data as DataFile.",
                                            );
                                        }
                                    },
                                    Err(e) => {
                                        self.show_error_dialog(&format!("File read error: {}", e));
                                    }
                                }
                            } else {
                                let ext = std::path::Path::new(&path_str)
                                    .extension()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let ext_with_dot = if !ext.is_empty() {
                                    format!(".{}", ext)
                                } else {
                                    "".to_string()
                                };
                                let candidates: Vec<_> = self
                                    .user_settings
                                    .conversion_scripts
                                    .iter()
                                    .cloned()
                                    .filter(|script| {
                                        script
                                            .extensions
                                            .iter()
                                            .any(|e| e.to_lowercase() == ext_with_dot)
                                    })
                                    .collect();
                                if candidates.is_empty() {
                                    self.show_error_dialog(&format!(
                                        "拡張子 {} に対応する変換スクリプトが設定されていません。",
                                        ext_with_dot
                                    ));
                                } else if candidates.len() == 1 {
                                    self.execute_conversion(&path_str, candidates[0].clone());
                                } else {
                                    self.pending_import_file = Some(path_str);
                                    self.pending_script_candidates = Some(candidates);
                                }
                            }
                        }
                    }

                    if ui.button("Exit").clicked() {
                        std::process::exit(0);
                    }
                });
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
            });
        });

        // 左側ペイン：各ファイルごとのシグナルツリー表示
        egui::SidePanel::left("group_panel")
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.open_files.is_empty() {
                        ui.label("No file loaded.");
                    } else {
                        for file_data in &mut self.open_files {
                            egui::CollapsingHeader::new(&file_data.file_name)
                                .default_open(true)
                                .show(ui, |ui| {
                                    let file_all_visible =
                                        file_data.signals.values().all(|sig| sig.visible);
                                    let mut file_toggle = file_all_visible;
                                    if ui.checkbox(&mut file_toggle, "Toggle All").changed() {
                                        for sig in file_data.signals.values_mut() {
                                            sig.visible = file_toggle;
                                        }
                                    }
                                    let mut group_keys: Vec<String> =
                                        file_data.groups.keys().cloned().collect();
                                    group_keys.sort();
                                    for group_key in group_keys {
                                        if let Some(group) = file_data.groups.get(&group_key) {
                                            let group_all_visible = group
                                                .signals
                                                .iter()
                                                .all(|s| file_data.signals[s].visible);
                                            egui::CollapsingHeader::new(&group.name)
                                                .default_open(false)
                                                .show(ui, |ui| {
                                                    let mut group_toggle = group_all_visible;
                                                    if ui
                                                        .checkbox(&mut group_toggle, "Toggle All")
                                                        .changed()
                                                    {
                                                        for s in &group.signals {
                                                            if let Some(sig) =
                                                                file_data.signals.get_mut(s)
                                                            {
                                                                sig.visible = group_toggle;
                                                            }
                                                        }
                                                    }
                                                    ui.indent("group_signals", |ui| {
                                                        for s in &group.signals {
                                                            if let Some(sig) =
                                                                file_data.signals.get_mut(s)
                                                            {
                                                                let mut check = sig.visible;
                                                                if ui
                                                                    .checkbox(&mut check, &sig.name)
                                                                    .changed()
                                                                {
                                                                    sig.visible = check;
                                                                }
                                                            }
                                                        }
                                                    });
                                                });
                                            ui.separator();
                                        }
                                    }
                                });
                        }
                    }
                });
            });

        // 中央ペイン：全ファイル・全グループ・全シグナルを左ペインと同じ順で列挙し、
        // 可視のものだけ順番に上から詰めて描画する
        egui::CentralPanel::default().show(ctx, |ui| {
            // グローバルな時刻範囲を計算
            let global_min_time = self
                .open_files
                .iter()
                .map(|f| f.min_time)
                .fold(f64::INFINITY, f64::min);
            let global_max_time = self
                .open_files
                .iter()
                .map(|f| f.max_time)
                .fold(0.0, f64::max);
            let global_min_time = if global_min_time == f64::INFINITY {
                0.0
            } else {
                global_min_time
            };
            let global_max_time = if global_max_time == 0.0 {
                10.0
            } else {
                global_max_time
            };

            // 左ペインの順序と同じく「ファイル→グループ→シグナル」で可視シグナルを抽出
            // → 上から順にオフセットを割り当てる
            let mut visible_signals = Vec::new(); // (label, color, intervals)
            let mut file_index = 0;
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

            for file_data in &self.open_files {
                let mut group_keys: Vec<String> = file_data.groups.keys().cloned().collect();
                group_keys.sort();
                // 好みで、ファイル名を色分けの単位にするならここでリセットしてもよい
                // 例: let mut color_idx = 0;
                for group_key in group_keys {
                    if let Some(group) = file_data.groups.get(&group_key) {
                        for s in &group.signals {
                            if let Some(sig) = file_data.signals.get(s) {
                                if sig.visible {
                                    // signal の表示ラベルは "ファイル名 → シグナル名" などお好みで
                                    let label = format!("{} / {}", file_data.file_name, sig.name);
                                    // ここではシグナルごとに適当にパレットから色を取る例
                                    // 実際にはシグナル固有の色があればそれを使っても良い
                                    // 例: let color_idx = (file_index + ???) % color_palette.len();
                                    let color_idx =
                                        (file_index + visible_signals.len()) % color_palette.len();
                                    let color = color_palette[color_idx];
                                    visible_signals.push((label, color, &sig.on_intervals));
                                }
                            }
                        }
                    }
                }
                file_index += 1;
            }

            // 上から詰めて描画するためにオフセットを割り当てる
            // 一番上が visible_signals[0]、次が visible_signals[1] ... という風に
            // ここでは「上を大きい数字、下を小さい数字」にする場合は逆順にしても良い
            let total = visible_signals.len();
            let mut offset_map = HashMap::new(); // y軸ラベル用
            let mut lines_to_draw = Vec::new();
            for (i, (label, color, intervals)) in visible_signals.into_iter().enumerate() {
                // i=0 を最上にする → y_offset = (total - i) * 2 - 1
                let y_offset = ((total - i) * 2 - 1) as f64;
                offset_map.insert(y_offset.round() as i32, label.clone());

                let line =
                    Self::build_digital_wave(intervals, global_min_time, global_max_time, y_offset)
                        .color(color)
                        .width(2.0)
                        .name(label);
                lines_to_draw.push(line);
            }

            egui_plot::Plot::new("global_digital_wave_plot")
                .min_size(ui.available_size())
                .include_x(global_min_time)
                .include_x(global_max_time)
                .x_axis_formatter(
                    |grid_mark: egui_plot::GridMark, _range: &RangeInclusive<f64>| {
                        let x = grid_mark.value;
                        let base_dt = Utc.timestamp_opt(0, 0).unwrap();
                        let dt = base_dt + Duration::milliseconds((x * 1000.0) as i64);
                        dt.naive_utc().format("%H:%M:%S%.3f").to_string()
                    },
                )
                .y_axis_formatter(
                    move |grid_mark: egui_plot::GridMark, _range: &RangeInclusive<f64>| {
                        let y = grid_mark.value;
                        let y_int = y.round() as i32;
                        offset_map.get(&y_int).cloned().unwrap_or_default()
                    },
                )
                .legend(Legend::default())
                .show(ui, |plot_ui: &mut PlotUi| {
                    for line in lines_to_draw {
                        plot_ui.line(line);
                    }
                });
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::WebOptions;
    eframe::start_web(
        "the_canvas_id",
        WebOptions::default(),
        Box::new(|_cc| Box::new(MyApp::new())),
    )
    .expect("failed to start eframe on the web");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = MyApp::new();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Log Analyzer",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )?;
    Ok(())
}
