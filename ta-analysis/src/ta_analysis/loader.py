from __future__ import annotations

import json
from pathlib import Path
from typing import Iterator

import pandas as pd

from ta_analysis.models import RawBacktestRow


def load_jsonl(path: str | Path) -> list[RawBacktestRow]:
    path = Path(path).expanduser()
    rows: list[RawBacktestRow] = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            data = json.loads(line)
            rows.append(RawBacktestRow(**data))
    return rows


def load_jsonl_df(path: str | Path) -> pd.DataFrame:
    rows = load_jsonl(path)
    records = [r.model_dump() for r in rows]
    df = pd.DataFrame.from_records(records)
    if not df.empty and "ts_ns" in df.columns:
        df["ts"] = pd.to_datetime(df["ts_ns"], unit="ns")
        df = df.set_index("ts").sort_index()
    return df


def iter_jsonl(path: str | Path) -> Iterator[RawBacktestRow]:
    path = Path(path).expanduser()
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            yield RawBacktestRow(**json.loads(line))
