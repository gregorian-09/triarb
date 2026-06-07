package com.triangulararb.dashboard.domain.model;

import java.util.List;

public record PnLSummary(
    double totalPnlUsdt,
    int totalTrades,
    int winCount,
    int lossCount,
    double winRatePct,
    double sharpeRatio,
    double maxDrawdownPct,
    double avgProfitBps,
    List<EquityPoint> equityCurve
) {}
