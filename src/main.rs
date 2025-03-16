#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use chrono::{Duration, TimeZone, Utc};
use eframe;
use egui;
use egui::Color32;
use egui_plot::{Legend, Line, PlotPoints, PlotUi};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::ops::RangeInclusive;
use std::process::Command;

// ユーザー設定で管理する変換スクリプトの設定
#[derive(Debug, Serialize, Deserialize, Clone)]
struct ConversionScriptSetting {
    name: String,
    script_path: String,
    // 拡張子のリスト（例: [".log", ".txt"]）―小文字で記述することを推奨
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
                // 例として .log と .txt ファイルに対応
                extensions: vec![".log".to_string(), ".txt".to_string()],
            }],
        }
    }
}

/// ログの1エントリ（.json の JSON）
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

    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64,
}

/// JSONファイル全体を表す構造体。時系列データと各シグナルの初期表示設定を含む。
#[derive(Debug, Deserialize, Serialize)]
struct DataFile {
    logs: Vec<LogEntry>,
    default_visibility: Option<Vec<VisibilityEntry>>,
}

/// 各シグナルの初期表示状態を表す
#[derive(Debug, Deserialize, Serialize)]
struct VisibilityEntry {
    group: String,
    name: String,
    visible: bool,
}

/// ON区間を表す
struct Interval {
    start: f64,
    end: f64,
}

/// 1信号のON区間等の情報
struct SignalData {
    name: String,
    y_offset: f64, // OFF時の基準位置
    on_intervals: Vec<Interval>,
    is_on: Option<f64>, // ONOFF の場合、ON開始時刻を記録
    visible: bool,      // 表示／非表示（default_visibility の定義に従う）
    color: Color32,     // 固定の色
}

/// 信号グループ（例: group1, group2）
struct GroupData {
    name: String,
    signals: Vec<String>,
}

/// 変換結果を保持する構造体
#[derive(Clone)]
struct ConversionResult {
    command: String,
    stdout: String,
    stderr: String,
    ok: bool,
    json_file: Option<String>,
}

/// メインアプリケーション。ここでは、ファイル名（例: file_a）を最上位グループとして保持します。
struct MyApp {
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    offset_to_name: HashMap<i32, String>,
    min_time: f64,
    max_time: f64,
    groups: HashMap<String, GroupData>,
    conversion_result: Option<ConversionResult>,
    error_dialog_message: Option<String>,
    // 各シグナルの初期表示状態：キーは (group, name) の組み合わせ
    visibility_defaults: HashMap<(String, String), bool>,
    // ファイル名（拡張子なし）を保持し、ファイル単位のグループとして利用する
    file_name: Option<String>,
    // ユーザー設定：python3 のパスや変換スクリプトの設定
    user_settings: UserSettings,
    // 設定ウィンドウの表示フラグ
    settings_open: bool,
    // Import 実行前に候補が複数あった場合、選択するための一時状態
    pending_import_file: Option<String>,
    pending_script_candidates: Option<Vec<ConversionScriptSetting>>,
}

impl MyApp {
    fn new() -> Self {
        let user_settings = Self::load_settings().unwrap_or_default();
        Self {
            logs: Vec::new(),
            signals: HashMap::new(),
            offset_to_name: HashMap::new(),
            min_time: 0.0,
            max_time: 10.0,
            groups: HashMap::new(),
            conversion_result: None,
            error_dialog_message: None,
            visibility_defaults: HashMap::new(),
            file_name: None,
            user_settings,
            settings_open: false,
            pending_import_file: None,
            pending_script_candidates: None,
        }
    }

    /// ユーザー設定ファイル (user_settings.json) を読み込む
    fn load_settings() -> Result<UserSettings, Box<dyn std::error::Error>> {
        let settings_file = "user_settings.json";
        if let Ok(content) = fs::read_to_string(settings_file) {
            let settings: UserSettings = serde_json::from_str(&content)?;
            Ok(settings)
        } else {
            Ok(UserSettings::default())
        }
    }

    /// エラーダイアログの表示
    fn show_error_dialog(&mut self, message: &str) {
        eprintln!("{}", message);
        self.error_dialog_message = Some(message.to_owned());
    }

    /// 指定の on_intervals からデジタル波形を生成する
    fn build_digital_wave(
        on_intervals: &Vec<Interval>,
        min_t: f64,
        max_t: f64,
        offset: f64,
    ) -> Line {
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

    /// ログやシグナル、グループ情報の再計算
    fn recalc(&mut self) {
        self.min_time = self.logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
        self.max_time = self.logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

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
                    y_offset: 0.0,
                    on_intervals: vec![],
                    is_on: None,
                    visible: false,
                    color: egui::Color32::WHITE,
                },
            );
        }

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

        for log in &self.logs {
            update_signal_data(&mut self.signals, log);
        }
        for sig in self.signals.values_mut() {
            merge_on_intervals(sig);
        }

        let mut group_keys: Vec<String> = self.groups.keys().cloned().collect();
        group_keys.sort();
        let mut ordered_signal_names = Vec::new();
        for gk in &group_keys {
            if let Some(group) = self.groups.get(gk) {
                for s in &group.signals {
                    ordered_signal_names.push(s.clone());
                }
            }
        }
        let total = ordered_signal_names.len();
        let color_palette = [
            egui::Color32::RED,
            egui::Color32::GREEN,
            egui::Color32::BLUE,
            egui::Color32::YELLOW,
            egui::Color32::LIGHT_BLUE,
            egui::Color32::LIGHT_GREEN,
            egui::Color32::WHITE,
            egui::Color32::GOLD,
        ];
        self.offset_to_name.clear();
        for (i, name) in ordered_signal_names.into_iter().enumerate() {
            let y_offset = ((total - i) * 2 - 1) as f64;
            let color = color_palette[i % color_palette.len()];
            if let Some(sig) = self.signals.get_mut(&name) {
                sig.y_offset = y_offset;
                sig.color = color;
            }
            self.offset_to_name.insert(y_offset as i32, name.clone());
        }
    }

    /// 実際に変換スクリプトを実行する関数
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
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());

        // エラーダイアログの表示
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

        // 変換結果ウィンドウの表示
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
                                            let mut logs = data_file.logs;
                                            for log in &mut logs {
                                                log.timestamp_num =
                                                    parse_timestamp_to_f64(&log.timestamp);
                                            }
                                            logs.sort_by(|a, b| {
                                                a.timestamp_num
                                                    .partial_cmp(&b.timestamp_num)
                                                    .unwrap()
                                            });
                                            self.logs = logs;
                                            self.visibility_defaults.clear();
                                            if let Some(defaults) = data_file.default_visibility {
                                                for entry in defaults {
                                                    self.visibility_defaults.insert(
                                                        (entry.group, entry.name),
                                                        entry.visible,
                                                    );
                                                }
                                            }
                                            self.file_name = Some(
                                                std::path::Path::new(json_path)
                                                    .file_stem()
                                                    .unwrap()
                                                    .to_string_lossy()
                                                    .to_string(),
                                            );
                                            self.recalc();
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
            // 一時的に借用する
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
                                            let mut logs = data_file.logs;
                                            for log in &mut logs {
                                                log.timestamp_num =
                                                    parse_timestamp_to_f64(&log.timestamp);
                                            }
                                            logs.sort_by(|a, b| {
                                                a.timestamp_num
                                                    .partial_cmp(&b.timestamp_num)
                                                    .unwrap()
                                            });
                                            self.logs = logs;
                                            self.visibility_defaults.clear();
                                            if let Some(defaults) = data_file.default_visibility {
                                                for entry in defaults {
                                                    self.visibility_defaults.insert(
                                                        (entry.group, entry.name),
                                                        entry.visible,
                                                    );
                                                }
                                            }
                                            self.file_name = Some(
                                                std::path::Path::new(&path_str)
                                                    .file_stem()
                                                    .unwrap()
                                                    .to_string_lossy()
                                                    .to_string(),
                                            );
                                            self.recalc();
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
                                            let mut logs = data_file.logs;
                                            for log in &mut logs {
                                                log.timestamp_num =
                                                    parse_timestamp_to_f64(&log.timestamp);
                                            }
                                            logs.sort_by(|a, b| {
                                                a.timestamp_num
                                                    .partial_cmp(&b.timestamp_num)
                                                    .unwrap()
                                            });
                                            self.logs = logs;
                                            self.visibility_defaults.clear();
                                            if let Some(defaults) = data_file.default_visibility {
                                                for entry in defaults {
                                                    self.visibility_defaults.insert(
                                                        (entry.group, entry.name),
                                                        entry.visible,
                                                    );
                                                }
                                            }
                                            self.file_name = Some(
                                                std::path::Path::new(&path_str)
                                                    .file_stem()
                                                    .unwrap()
                                                    .to_string_lossy()
                                                    .to_string(),
                                            );
                                            self.recalc();
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
                                // 非 json ファイルの場合、ユーザー設定から拡張子に合致する変換スクリプトを選ぶ
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

        // 左側パネル：ファイルグループ→各グループ→シグナルのネスト表示
        egui::SidePanel::left("group_panel")
            .resizable(true)
            .show(ctx, |ui| {
                //ui.heading("Signals");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(ref file_name) = self.file_name {
                        egui::CollapsingHeader::new(file_name)
                            .default_open(true)
                            .show(ui, |ui| {
                                let file_all_visible = self.signals.values().all(|sig| sig.visible);
                                let mut file_toggle = file_all_visible;
                                if ui.checkbox(&mut file_toggle, "Toggle All").changed() {
                                    for sig in self.signals.values_mut() {
                                        sig.visible = file_toggle;
                                    }
                                }
                                let mut group_keys: Vec<String> =
                                    self.groups.keys().cloned().collect();
                                group_keys.sort();
                                for group_key in group_keys {
                                    if let Some(group) = self.groups.get(&group_key) {
                                        let group_all_visible =
                                            group.signals.iter().all(|s| self.signals[s].visible);
                                        egui::CollapsingHeader::new(&group.name)
                                            .default_open(false)
                                            .show(ui, |ui| {
                                                let mut group_toggle = group_all_visible;
                                                if ui
                                                    .checkbox(&mut group_toggle, "Toggle All")
                                                    .changed()
                                                {
                                                    for s in &group.signals {
                                                        if let Some(sig) = self.signals.get_mut(s) {
                                                            sig.visible = group_toggle;
                                                        }
                                                    }
                                                }
                                                ui.indent("group_signals", |ui| {
                                                    for s in &group.signals {
                                                        if let Some(sig) = self.signals.get_mut(s) {
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
                    } else {
                        ui.label("No file loaded.");
                    }
                });
            });

        {
            let mut group_keys: Vec<String> = self.groups.keys().cloned().collect();
            group_keys.sort();
            let mut visible_signals_in_order = Vec::new();
            for group_key in group_keys {
                if let Some(group) = self.groups.get(&group_key) {
                    for s in &group.signals {
                        if let Some(sig) = self.signals.get(s) {
                            if sig.visible {
                                visible_signals_in_order.push(s.clone());
                            }
                        }
                    }
                }
            }
            let total_visible = visible_signals_in_order.len();
            for (i, s) in visible_signals_in_order.iter().enumerate() {
                let offset = ((total_visible - i) * 2 - 1) as f64;
                if let Some(sig) = self.signals.get_mut(s) {
                    sig.y_offset = offset;
                }
            }
            self.offset_to_name.clear();
            for (name, sig) in self.signals.iter() {
                if sig.visible {
                    self.offset_to_name
                        .insert(sig.y_offset as i32, name.clone());
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            //ui.heading("Log Analyzer");
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
            //ui.label("Timeline (Digital Waveform)");
            let legend = Legend::default();
            let offset_to_name = self.offset_to_name.clone();
            egui_plot::Plot::new("digital_wave_plot")
                .min_size(ui.available_size())
                .include_x(self.min_time)
                .include_x(self.max_time)
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
                        offset_to_name
                            .get(&y_int)
                            .cloned()
                            .unwrap_or_else(|| "".to_string())
                    },
                )
                .legend(legend)
                .show(ui, |plot_ui: &mut PlotUi| {
                    let mut group_keys: Vec<String> = self.groups.keys().cloned().collect();
                    group_keys.sort();
                    let mut draw_index = 0;
                    for group_key in group_keys {
                        if let Some(group) = self.groups.get(&group_key) {
                            for signal_name in &group.signals {
                                if let Some(signal_data) = self.signals.get(signal_name) {
                                    if signal_data.visible {
                                        let wave_line = Self::build_digital_wave(
                                            &signal_data.on_intervals,
                                            self.min_time,
                                            self.max_time,
                                            signal_data.y_offset,
                                        );
                                        let legend_label =
                                            format!("{:02}: {}", draw_index, signal_data.name);
                                        plot_ui.line(
                                            wave_line
                                                .color(signal_data.color)
                                                .width(2.0)
                                                .name(legend_label),
                                        );
                                        draw_index += 1;
                                    }
                                }
                            }
                        }
                    }
                });
        });
    }
}

/// ISO8601 のタイムスタンプ文字列を f64 (Unix epoch 秒) に変換する
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

/// ログから各信号の on_intervals を更新する
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

/// 重なっている interval をマージする
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
    let app = MyApp::new();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Log Analyzer",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )?;
    Ok(())
}
