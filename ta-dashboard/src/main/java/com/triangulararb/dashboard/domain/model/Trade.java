package com.triangulararb.dashboard.domain.model;

import java.time.Instant;

public record Trade(
    String id,
    Instant timestamp,
    String legA,
    String legB,
    String legC,
    double expectedProfitBps,
    double actualProfitBps,
    double pnlUsdt
) {}
