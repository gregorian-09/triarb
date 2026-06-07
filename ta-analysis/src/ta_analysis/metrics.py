from __future__ import annotations

import numpy as np
import pandas as pd

from ta_analysis.models import BacktestSummary


def compute_equity_curve(
    trades: pd.DataFrame,
    initial_capital: float = 10_000.0,
    trade_size_usdt: float = 100.0,
) -> pd.Series:
    if trades.empty:
        return pd.Series(dtype=float)

    executed = trades[trades["executed"]].copy()
    if executed.empty:
        return pd.Series(initial_capital, index=trades.index[:1])

    executed["pnl_usdt"] = executed["expected_profit_bps"] / 10_000.0 * trade_size_usdt
    cumulative = executed["pnl_usdt"].cumsum()
    equity = cumulative + initial_capital
    return equity


def compute_drawdown(equity: pd.Series) -> pd.Series:
    if equity.empty:
        return pd.Series(dtype=float)
    rolling_max = equity.expanding().max()
    drawdown = (equity - rolling_max) / rolling_max * 100
    return drawdown


def compute_sharpe(equity: pd.Series, risk_free_rate: float = 0.0) -> float:
    if len(equity) < 2:
        return 0.0
    returns = equity.pct_change().dropna()
    if returns.std() == 0:
        return 0.0
    excess = returns - risk_free_rate / (365 * 24 * 60 * 60)
    return float(np.sqrt(len(returns)) * excess.mean() / returns.std())


def compute_summary(
    trades: pd.DataFrame,
    initial_capital: float = 10_000.0,
    trade_size_usdt: float = 100.0,
) -> BacktestSummary:
    if trades.empty:
        return BacktestSummary(
            total_opportunities=0,
            executed_trades=0,
            win_count=0,
            loss_count=0,
            total_pnl_usdt=0.0,
            sharpe_ratio=0.0,
            max_drawdown_pct=0.0,
            win_rate=0.0,
            avg_profit_bps=0.0,
        )

    total_opps = len(trades)
    executed = trades[trades["executed"]]
    exec_count = len(executed)

    if exec_count == 0:
        return BacktestSummary(
            total_opportunities=total_opps,
            executed_trades=0,
            win_count=0,
            loss_count=0,
            total_pnl_usdt=0.0,
            sharpe_ratio=0.0,
            max_drawdown_pct=0.0,
            win_rate=0.0,
            avg_profit_bps=float(trades["expected_profit_bps"].mean()),
        )

    equity = compute_equity_curve(trades, initial_capital, trade_size_usdt)
    drawdown = compute_drawdown(equity)

    wins = executed[executed["expected_profit_bps"] > 0]
    losses = executed[executed["expected_profit_bps"] <= 0]

    total_pnl = (executed["expected_profit_bps"] / 10_000.0 * trade_size_usdt).sum()

    return BacktestSummary(
        total_opportunities=total_opps,
        executed_trades=exec_count,
        win_count=len(wins),
        loss_count=len(losses),
        total_pnl_usdt=round(total_pnl, 2),
        sharpe_ratio=round(compute_sharpe(equity), 4),
        max_drawdown_pct=round(drawdown.min(), 2) if not drawdown.empty else 0.0,
        win_rate=round(len(wins) / exec_count * 100, 2) if exec_count > 0 else 0.0,
        avg_profit_bps=round(float(executed["expected_profit_bps"].mean()), 2),
    )
