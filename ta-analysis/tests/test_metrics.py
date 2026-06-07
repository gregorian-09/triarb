from __future__ import annotations

import pandas as pd
import pytest

from ta_analysis.metrics import compute_equity_curve, compute_drawdown, compute_sharpe, compute_summary


def _sample_trades() -> pd.DataFrame:
    return pd.DataFrame.from_records(
        [
            {
                "ts_ns": 1_000_000_000,
                "leg_a": "BTCUSDT",
                "leg_b": "ETHBTC",
                "leg_c": "ETHUSDT",
                "profit_bps": 15.0,
                "expected_profit_bps": 12.0,
                "executed": True,
            },
            {
                "ts_ns": 2_000_000_000,
                "leg_a": "SOLUSDT",
                "leg_b": "ETHSOL",
                "leg_c": "ETHUSDT",
                "profit_bps": 8.0,
                "expected_profit_bps": 5.0,
                "executed": True,
            },
            {
                "ts_ns": 3_000_000_000,
                "leg_a": "BNBUSDT",
                "leg_b": "ETHBNB",
                "leg_c": "ETHUSDT",
                "profit_bps": -3.0,
                "expected_profit_bps": -5.0,
                "executed": True,
            },
        ]
    )


def test_compute_equity_curve():
    trades = _sample_trades()
    equity = compute_equity_curve(trades, initial_capital=10_000.0, trade_size_usdt=100.0)
    assert not equity.empty
    assert round(equity.iloc[0], 2) == 10_000.12


def test_compute_drawdown():
    equity = pd.Series([100, 110, 105, 120])
    dd = compute_drawdown(equity)
    assert dd.iloc[0] == 0.0
    assert dd.iloc[2] < 0


def test_compute_sharpe():
    equity = pd.Series([100, 101, 102, 103, 104, 105])
    sharpe = compute_sharpe(equity)
    assert sharpe > 0


def test_compute_summary():
    trades = _sample_trades()
    summary = compute_summary(trades, initial_capital=10_000.0, trade_size_usdt=100.0)
    assert summary.total_opportunities == 3
    assert summary.executed_trades == 3
    assert summary.win_count == 2
    assert summary.loss_count == 1
    assert summary.win_rate == 66.67


def test_empty_trades():
    empty = pd.DataFrame()
    summary = compute_summary(empty)
    assert summary.total_opportunities == 0
    assert summary.executed_trades == 0
