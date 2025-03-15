#!/usr/bin/env python3
import re
import sys
import json
import os
from datetime import datetime, timezone

# グローバルリスト: 最終的にレコードを蓄積する
records = []

def add_record(timestamp, type_val, group, name, value, comment):
    """
    サブルーチンB:
    各フィールド（timestamp, type, group, name, value, comment）を直接引数として受け取り、
    レコードを生成してグローバル変数 records に追加する。
    """
    record = {
        "timestamp": timestamp,
        "type": type_val,
        "group": group,
        "name": name,
        "value": value,
        "comment": comment
    }
    records.append(record)

def process_line_sub(line, timestamp=None):
    """
    サブルーチンA:
    複数の正規表現による処理を順次実行し、合致した場合はサブルーチンB (add_record) を呼び出す。
    ここでは例として「name: comment」形式のパターンを処理する。
    """
    patterns = [
#        re.compile(r'^(?P<name>\S+):\s+(?P<comment>.+)$'),
        re.compile(r'^\[.*?\]\s+(?P<name>[^:]+):\s+(?P<comment>.+)$'),
    ]
    
    for pat in patterns:
        m = pat.search(line)
        if m:
            add_record(
                timestamp if timestamp is not None else "",
                "PULSE",
                "group1",
                m.group("name"),
                400,
                m.group("comment")
            )
            # 複数パターンにヒットする可能性があるため、ループは継続

def main():
    if len(sys.argv) != 2:
        print("Usage: python convert.py input.log")
        sys.exit(1)
    
    input_file = sys.argv[1]
    
    # ファイルの全行を読み込む
    with open(input_file, 'r', encoding='utf-8') as f:
        lines = f.readlines()

    # 先頭にある角括弧タイムスタンプ（例: [05:30:56.917948]）を除去
    bracket_ts_re = re.compile(r'^\[\d{2}:\d{2}:\d{2}\.\d+\]\s*')
    # 行の先頭にある前半部から、ISO8601形式のタイムスタンプのみを "ts" としてキャプチャする正規表現
    # 例: "2025-03-11T05:30:54.867Z:I:0x00100000:" → ts: "2025-03-11T05:30:54.867Z"
    prefix_re = re.compile(
        r'^(?P<ts>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z):[^:]+:[^:]+:\s*(?P<rest>.*)$'
    )
    # 指定日時（この例では2025年1月1日以降）のみ処理するための基準日時（offset-aware）
    cutoff_date = datetime(2025, 1, 1, tzinfo=timezone.utc)

    for line in lines:
        line = line.strip()
        if not line:
            continue
        
        # 先頭の角括弧タイムスタンプを除去
        line = bracket_ts_re.sub("", line)
        
        m = prefix_re.match(line)
        if m:
            # "ts" グループでISO8601形式のタイムスタンプ全体を取得
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
#        else:
#            process_line_sub(line)
    
    output_file = os.path.splitext(input_file)[0] + ".json"
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(records, f, indent=2)
    
    print(f"Converted {input_file} to {output_file}")

if __name__ == '__main__':
    main()
