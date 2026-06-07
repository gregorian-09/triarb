package com.triangulararb.dashboard.domain.model;

import java.time.Instant;

public record Opportunity(
    Instant timestamp,
    String legA,
    String legB,
    String legC,
    double expectedProfitBps
) {}
