use chrono::{Duration, NaiveDateTime, TimeZone, Utc};
use eframe;
use egui;
use egui::Color32;
use egui_plot::{Legend, Line, Plot, PlotPoints, PlotUi};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::ops::RangeInclusive;
use std::process::Command;

/// ログの1エントリ（.json の JSON）
#[derive(Debug, Deserialize, Serialize)]
struct LogEntry {
    timestamp: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    // グループ化用フィールド
    #[serde(default)]
    group: Option<String>,
    value: Value,
    comment: Option<String>,

    #[serde(skip_serializing, skip_deserializing)]
    timestamp_num: f64,
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
    visible: bool,      // 表示／非表示
    color: Color32,     // 固定の色
}

/// 信号グループ
struct GroupData {
    name: String,
    signals: Vec<String>, // 信号名リスト
}

/// メインアプリケーション
struct MyApp {
    logs: Vec<LogEntry>,
    signals: HashMap<String, SignalData>,
    offset_to_name: HashMap<i32, String>,
    min_time: f64,
    max_time: f64,
    groups: HashMap<String, GroupData>,
}

impl MyApp {
    fn recalc(&mut self) {
        // タイムレンジの再計算
        self.min_time = self.logs.first().map(|x| x.timestamp_num).unwrap_or(0.0);
        self.max_time = self.logs.last().map(|x| x.timestamp_num).unwrap_or(10.0);

        // ユニークなシグナル名の抽出と signals の再初期化
        use std::collections::BTreeSet;
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
                    visible: true,
                    color: egui::Color32::WHITE,
                },
            );
        }

        // グループの再構築
        self.groups.clear();
        let mut signal_to_group = std::collections::HashMap::new();
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
        for (signal_name, group_name) in signal_to_group {
            if let Some(g) = self.groups.get_mut(&group_name) {
                if !g.signals.contains(&signal_name) {
                    g.signals.push(signal_name);
                }
            }
        }
        for g in self.groups.values_mut() {
            g.signals.sort();
        }

        // ログデータからシグナルの on_intervals の更新
        for log in &self.logs {
            update_signal_data(&mut self.signals, log);
        }
        for sig in self.signals.values_mut() {
            merge_on_intervals(sig);
        }

        // シグナルの表示順（y_offset）と凡例用の offset_to_name の再計算
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
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Menu
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open").clicked() {
                        ui.close_menu(); // メニューを閉じる
                        if let Some(path) = FileDialog::new().pick_file() {
                            let path_str = path.to_string_lossy().to_string();
                            if path_str.to_lowercase().ends_with(".json") {
                                match std::fs::read_to_string(&path_str) {
                                    Ok(data) => {
                                        if let Ok(mut logs) =
                                            serde_json::from_str::<Vec<LogEntry>>(&data)
                                        {
                                            // ログデータの前処理
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
                                            // ここで再計算を実行
                                            self.recalc();
                                        } else {
                                            eprintln!("JSON のパースに失敗しました");
                                        }
                                    }
                                    Err(e) => eprintln!("ファイル読み込みエラー: {}", e),
                                }
                            } else {
                                // .json 以外のファイルの場合の処理
                                // ...
                            }
                        }
                    }
                    if ui.button("Import").clicked() {
                        ui.close_menu();
                        if let Some(path) = FileDialog::new().pick_file() {
                            let path_str = path.to_string_lossy().to_string();
                            // .json 以外の場合は変換処理を実行
                            if !path_str.to_lowercase().ends_with(".json") {
                                // 変換スクリプトの実行（python3 を使用して scripts/convert.py を呼び出す例）
                                let output = Command::new("python3")
                                    .arg("scripts/convert.py")
                                    .arg(&path_str)
                                    .output();

                                if let Ok(output) = output {
                                    if output.status.success() {
                                        // 入力ファイル名の拡張子を .json に変更して生成ファイル名を取得
                                        let json_path = std::path::Path::new(&path_str)
                                            .with_extension("json")
                                            .to_string_lossy()
                                            .to_string();
                                        // 生成された JSON ファイルを読み込む
                                        if let Ok(data) = std::fs::read_to_string(&json_path) {
                                            if let Ok(mut logs) =
                                                serde_json::from_str::<Vec<LogEntry>>(&data)
                                            {
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
                                                self.recalc();
                                            } else {
                                                eprintln!("JSON のパースに失敗しました");
                                            }
                                        } else {
                                            eprintln!(
                                                "生成されたファイル {} の読み込みに失敗しました",
                                                json_path
                                            );
                                        }
                                    } else {
                                        eprintln!(
                                            "変換スクリプトの実行に失敗しました: {:?}",
                                            output
                                        );
                                    }
                                } else {
                                    eprintln!("変換スクリプトを実行できませんでした");
                                }
                            } else {
                                // すでに .json ファイルの場合は、Open の処理と同様に読み込みます
                                if let Ok(data) = std::fs::read_to_string(&path_str) {
                                    if let Ok(mut logs) =
                                        serde_json::from_str::<Vec<LogEntry>>(&data)
                                    {
                                        for log in &mut logs {
                                            log.timestamp_num =
                                                parse_timestamp_to_f64(&log.timestamp);
                                        }
                                        logs.sort_by(|a, b| {
                                            a.timestamp_num.partial_cmp(&b.timestamp_num).unwrap()
                                        });
                                        self.logs = logs;
                                        self.recalc();
                                    } else {
                                        eprintln!("JSON のパースに失敗しました");
                                    }
                                } else {
                                    eprintln!("ファイル読み込みエラー: {}", path_str);
                                }
                            }
                        }
                    }

                    if ui.button("Exit").clicked() {
                        std::process::exit(0);
                    }
                });
            });
        });
        // 左側パネル：グループ／信号のチェックボックス
        egui::SidePanel::left("group_panel")
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Groups");

                let mut group_keys: Vec<String> = self.groups.keys().cloned().collect();
                group_keys.sort();

                for group_key in group_keys {
                    let group = self.groups.get_mut(&group_key).unwrap();

                    // 表示中の信号数をカウント
                    let visible_count = group
                        .signals
                        .iter()
                        .filter(|s| self.signals[*s].visible)
                        .count();
                    let mut group_check = visible_count > 0;
                    let group_response = ui.checkbox(&mut group_check, &group.name);

                    if group_response.changed() {
                        for s in &group.signals {
                            if let Some(sig) = self.signals.get_mut(s) {
                                sig.visible = group_check;
                            }
                        }
                    }

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

        // ★★★ 表示中のシグナルだけを抽出し、y_offset と offset_to_name を再計算 ★★★
        {
            // グループ順にソートして、可視信号を抽出
            let mut group_keys: Vec<String> = self.groups.keys().cloned().collect();
            group_keys.sort();

            let mut visible_signals_in_order = Vec::new();
            for group_key in group_keys {
                let group = &self.groups[&group_key];
                for s in &group.signals {
                    if let Some(sig) = self.signals.get(s) {
                        if sig.visible {
                            visible_signals_in_order.push(s.clone());
                        }
                    }
                }
            }

            let total_visible = visible_signals_in_order.len();
            for (i, s) in visible_signals_in_order.iter().enumerate() {
                // 上から順に配置（例: 上位が高い y 値）
                let offset = ((total_visible - i) * 2 - 1) as f64;
                if let Some(sig) = self.signals.get_mut(s) {
                    sig.y_offset = offset;
                }
            }

            // offset_to_name を再構築（可視信号のみ）
            self.offset_to_name.clear();
            for (name, sig) in self.signals.iter() {
                if sig.visible {
                    self.offset_to_name
                        .insert(sig.y_offset as i32, name.clone());
                }
            }
        }

        // 中央パネル：波形描画とログ表示
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("My Rust EGUI App - Single-Step ON/OFF Waveform");

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

            // 凡例
            let legend = Legend::default();

            // プロット描画
            let offset_to_name = self.offset_to_name.clone();

            Plot::new("digital_wave_plot")
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
                        let group = &self.groups[&group_key];
                        for signal_name in &group.signals {
                            if let Some(signal_data) = self.signals.get(signal_name) {
                                if signal_data.visible {
                                    let wave_line = build_digital_wave(
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
                });
        });
    }
}

/// 指定の on_intervals からデジタル波形を生成する  
/// OFF = y_offset, ON = y_offset + 1
fn build_digital_wave(on_intervals: &Vec<Interval>, min_t: f64, max_t: f64, offset: f64) -> Line {
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

/// ISO8601 のタイムスタンプ文字列を f64 (Unix epoch 秒) に変換する
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
    // 起動時は空のログ・信号・グループで初期化する
    let app = MyApp {
        logs: Vec::new(),
        signals: HashMap::new(),
        offset_to_name: HashMap::new(),
        min_time: 0.0,
        max_time: 10.0,
        groups: HashMap::new(),
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "My Rust EGUI App - Single-Step ON/OFF Waveform",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )?;
    Ok(())
}
