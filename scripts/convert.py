#!/usr/bin/env python3
import re
import sys
import json
import os
from datetime import datetime, timezone

# グローバルリスト: ログレコードを蓄積する
records = []
# グローバル辞書: (group, name) ペアで、表示するものだけ true を設定する
default_visibilities = {}

def add_record(timestamp, type_val, group, name, value, comment):
    """
    各フィールドを受け取りレコードを生成し、records に追加する。
    また、コメントに "[default_visible]" が含まれていれば、
    該当の (group, name) ペアを default_visibilities に true として登録する。
    """
    if not timestamp:
        return

    record = {
        "timestamp": timestamp,
        "type": type_val,
        "group": group,
        "name": name,
        "value": value,
        "comment": comment
    }
    records.append(record)
    
    # コメントに "[default_visible]" があれば、表示対象として true を設定
    if comment and "[default_visible]" in comment:
        default_visibilities[(group, name)] = True

def handle_pattern1(m, timestamp):
    # 1 番目のパターンの処理
    name = m.group("name")
    comment = m.group("comment")
    # "hoge.c-100" のような形式の場合、'-' で分割して先頭部分をグループ名とする
    group_match = re.match(r'^(?P<group>[^-]+\.c)', name)
    if group_match:
        group_val = group_match.group("group")
    else:
        group_val = "group1"
    
    add_record(
        timestamp if timestamp is not None else "",
        "PULSE",
        group_val,
        name,
        400,
        comment
    )

def handle_pattern2(m, timestamp):
    # 2 番目のパターンの処理（必要に応じて実装）
    name = m.group("name")
    priority = m.group("priority")
    comment = m.group("comment")
    if "hoge.c" in name:
        add_record(
            timestamp if timestamp is not None else "",
            "PULSE",
            "group1",
            name,
            400,
            comment
        )

def process_line_sub(line, timestamp=None):
    """
    複数の正規表現を試し、合致した場合は add_record を呼び出す
    """
    pattern_handlers = [
        (re.compile(r'^\[.*?\]\s+(?P<name>[^:]+):\s+(?P<comment>.+)$'), handle_pattern1),
        # (re.compile(r'^\[(?P<priority>.+)\]\s+(?P<name>[^:]+):\s+(?P<comment>.+)$'), handle_pattern2),
    ]

    for pat, handler in pattern_handlers:
        m = pat.search(line)
        if m:
            handler(m, timestamp)
            # 複数パターンにヒットする可能性があるため、ループは継続

def main():
    if len(sys.argv) != 2:
        print("Usage: python convert.py input.log")
        sys.exit(1)
    
    input_file = sys.argv[1]
    
    # ファイル全行を読み込み
    with open(input_file, 'r', encoding='utf-8') as f:
        lines = f.readlines()

    # 角括弧タイムスタンプ（例: [05:30:56.917948]）の除去用正規表現
    bracket_ts_re = re.compile(r'^\[\d{2}:\d{2}:\d{2}\.\d+\]\s*')
    # ISO8601 タイムスタンプをキャプチャする正規表現
    prefix_re = re.compile(
        r'^(?P<ts>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z):[^:]+:[^:]+:\s*(?P<rest>.*)$'
    )
    # 2025年1月1日以降のデータのみ処理するための基準日時
    cutoff_date = datetime(2025, 1, 1, tzinfo=timezone.utc)

    for line in lines:
        line = line.strip()
        if not line:
            continue
        
        # 角括弧タイムスタンプの除去
        line = bracket_ts_re.sub("", line)
        
        m = prefix_re.match(line)
        if m:
            ts_extracted = m.group("ts")
            try:
                dt = datetime.fromisoformat(ts_extracted.replace("Z", "+00:00"))
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=timezone.utc)
            except ValueError:
                print(f"Warning: Unable to parse timestamp: {ts_extracted}", file=sys.stderr)
                continue

            # 指定日時より前のデータはスキップ
            if dt < cutoff_date:
                continue

            rest = m.group("rest")
            process_line_sub(rest, ts_extracted)
        else:
            process_line_sub(line)
    
    output_file = os.path.splitext(input_file)[0] + ".json"
    output = {
        "logs": records,
        # default_visibility では、true として登録されたものだけ出力する
        "default_visibility": [
            {"group": key[0], "name": key[1], "visible": True}
            for key, value in default_visibilities.items() if value
        ]
    }
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(output, f, indent=2)
    
    print(f"Converted {input_file} to {output_file}")

if __name__ == '__main__':
    main()
