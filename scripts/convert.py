#!/usr/bin/env python3
import re
import sys
import json
import os
from datetime import datetime, timezone

def convert_log_to_json(input_file):
    # 正規表現パターン:
    #   - 行の先頭に「[」から「]」までの部分（および後続の空白）を任意でマッチ(なくてもよい)
    #   - 最初の部分でタイムスタンプを (\S+)
    #   - その後の2フィールドを無視
    #   - "[INFO]" などのログレベルを含む部分の後、
    #   - コロン区切りで name をキャプチャし、
    #   - その後の全体を comment としてキャプチャします。
    pattern = re.compile(
        r'^(?:\[[^\]]+\]\s+)?(?P<timestamp>\S+):\S+:\S+:\[.*?\]\s+(?P<name>[^:]+):\s+(?P<comment>.+)$'
    )
    
    records = []
    cutoff_date = datetime(2025, 1, 1, tzinfo=timezone.utc)

    with open(input_file, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            m = pattern.match(line)
            if m:
                timestamp_str = m.group("timestamp")
                try:
                    # "Z" を "+00:00" に置換してからパース（これによりoffset-awareなdatetimeが得られる）
                    ts = datetime.fromisoformat(timestamp_str.replace("Z", "+00:00"))
                except ValueError:
                    print(f"Warning: Unable to parse timestamp: {timestamp_str}", file=sys.stderr)
                    continue

                # 指定日時より前のデータはスキップ
                if ts < cutoff_date:
                    continue

                record = {
                    "timestamp": timestamp_str,
                    "type": "PULSE",
                    "group": "group1",
                    "name": m.group("name"),
                    "value": 400,
                    "comment": m.group("comment")
                }
                records.append(record)
            else:
                # 正規表現に合致しない行があればスキップするか、エラー出力する
                print(f"Warning: Unable to parse line: {line}", file=sys.stderr)
    return records

if __name__ == '__main__':
    if len(sys.argv) != 2:
        print("Usage: python convert.py input.log")
        sys.exit(1)
    
    input_file = sys.argv[1]
    records = convert_log_to_json(input_file)
    
    # 入力ファイルの拡張子を .json に変更
    output_file = os.path.splitext(input_file)[0] + ".json"
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(records, f, indent=2)
    
    print(f"Converted {input_file} to {output_file}")
