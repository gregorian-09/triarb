from pydantic import BaseModel, Field
from typing import Optional


class RawTrade(BaseModel):
    ts_ns: int
    symbol: str
    side: str
    price: float
    size: float
    profit_bps: Optional[float] = None


class RawBacktestRow(BaseModel):
    ts_ns: int
    leg_a_symbol: str = Field(alias="leg_a")
    leg_b_symbol: str = Field(alias="leg_b")
    leg_c_symbol: str = Field(alias="leg_c")
    profit_bps: float
    expected_profit_bps: float
    executed: bool = False
    fill_price_a: Optional[float] = None
    fill_price_b: Optional[float] = None
    fill_price_c: Optional[float] = None
    pnl_usdt: Optional[float] = None


class EquityPoint(BaseModel):
    ts_ns: int
    equity: float
    drawdown: float = 0.0


class BacktestSummary(BaseModel):
    total_opportunities: int
    executed_trades: int
    win_count: int
    loss_count: int
    total_pnl_usdt: float
    sharpe_ratio: float
    max_drawdown_pct: float
    win_rate: float
    avg_profit_bps: float
    total_fees_paid: float = 0.0
