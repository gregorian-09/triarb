from pydantic import BaseModel, Field, field_validator
from typing import Optional
from pathlib import Path


class AnalysisConfig(BaseModel):
    data_path: Path = Field(default=Path("backtest_results.jsonl"))
    min_profit_bps: float = Field(default=10.0, ge=0.0)
    fee_taker_bps: float = Field(default=10.0, ge=0.0)
    initial_capital: float = Field(default=10_000.0, gt=0.0)
    trade_size_usdt: float = Field(default=100.0, gt=0.0)

    @field_validator("data_path", mode="before")
    @classmethod
    def resolve_path(cls, v: str | Path) -> Path:
        return Path(v).expanduser().resolve()


class OptimizationRange(BaseModel):
    min_profit_bps_min: float = 1.0
    min_profit_bps_max: float = 50.0
    min_profit_bps_step: float = 1.0
    fee_taker_bps_values: list[float] = Field(default=[5.0, 7.5, 10.0])


class BacktestConfig(BaseModel):
    analysis: AnalysisConfig = AnalysisConfig()
    optimization: OptimizationRange = OptimizationRange()
