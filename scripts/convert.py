#!/usr/bin/env python3
import re
import sys
import json
import os

def convert_log_to_ulg(input_file):
    # 正規表現パターン:
    #   - 最初の部分でタイムスタンプを (\S+)
    #   - その後の2フィールドを無視
    #   - "[INFO]" などのログレベルを含む部分の後、
    #   - コロン区切りで name をキャプチャし、
    #   - その後の全体を comment としてキャプチャします。
    pattern = re.compile(
        r'^(?P<timestamp>\S+):\S+:\S+:\[.*?\]\s+(?P<name>[^:]+):\s+(?P<comment>.+)$'
    )
    
    records = []
    with open(input_file, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            m = pattern.match(line)
            if m:
                record = {
                    "timestamp": m.group("timestamp"),
                    "type": "PULSE",  # 固定値
                    "group": "group1",  # 固定値
                    "name": m.group("name"),
                    "value": 400,    # 固定値
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
    records = convert_log_to_ulg(input_file)
    
    # 入力ファイルの拡張子を .ulg に変更
    output_file = os.path.splitext(input_file)[0] + ".ulg"
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(records, f, indent=2)
    
    print(f"Converted {input_file} to {output_file}")
