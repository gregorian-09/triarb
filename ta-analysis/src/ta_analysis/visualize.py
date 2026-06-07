from __future__ import annotations

import pandas as pd
import plotly.graph_objects as go
from plotly.subplots import make_subplots

from ta_analysis.metrics import compute_drawdown, compute_equity_curve


def plot_equity_curve(
    trades: pd.DataFrame,
    initial_capital: float = 10_000.0,
    trade_size_usdt: float = 100.0,
    title: str = "Backtest Equity Curve",
) -> go.Figure:
    equity = compute_equity_curve(trades, initial_capital, trade_size_usdt)
    drawdown = compute_drawdown(equity)

    fig = make_subplots(
        rows=2,
        cols=1,
        shared_xaxes=True,
        vertical_spacing=0.05,
        row_heights=[0.7, 0.3],
        subplot_titles=(title, "Drawdown %"),
    )

    fig.add_trace(
        go.Scatter(x=equity.index, y=equity.values, mode="lines", name="Equity"),
        row=1,
        col=1,
    )
    fig.add_trace(
        go.Scatter(
            x=drawdown.index,
            y=drawdown.values,
            mode="lines",
            name="Drawdown",
            fill="tozeroy",
            line=dict(color="red"),
        ),
        row=2,
        col=1,
    )

    fig.update_layout(height=600, showlegend=False, hovermode="x unified")
    fig.update_yaxes(title_text="Equity (USDT)", row=1, col=1)
    fig.update_yaxes(title_text="Drawdown %", row=2, col=1)
    return fig


def plot_trade_scatter(trades: pd.DataFrame) -> go.Figure:
    executed = trades[trades["executed"]].copy()
    if executed.empty:
        return go.Figure()

    executed["pnl"] = executed["expected_profit_bps"] / 10_000.0 * 100
    colors = ["green" if pnl > 0 else "red" for pnl in executed["pnl"]]

    fig = go.Figure()
    fig.add_trace(
        go.Scatter(
            x=executed.index,
            y=executed["expected_profit_bps"],
            mode="markers",
            marker=dict(color=colors, size=6),
            name="Trades",
        )
    )
    fig.add_hline(y=0, line_dash="dash", line_color="gray")
    fig.update_layout(
        title="Trade Profit (bps)",
        xaxis_title="Time",
        yaxis_title="Profit (bps)",
        hovermode="x",
    )
    return fig


def plot_opportunity_distribution(trades: pd.DataFrame) -> go.Figure:
    fig = go.Figure()
    fig.add_trace(
        go.Histogram(
            x=trades["expected_profit_bps"],
            nbinsx=50,
            name="Opportunities",
        )
    )
    fig.update_layout(
        title="Distribution of Expected Profit (bps)",
        xaxis_title="Profit (bps)",
        yaxis_title="Frequency",
        bargap=0.05,
    )
    return fig
