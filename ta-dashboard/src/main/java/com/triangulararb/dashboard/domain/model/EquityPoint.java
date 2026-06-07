package com.triangulararb.dashboard.domain.model;

import java.time.Instant;

public record EquityPoint(
    Instant timestamp,
    double equity,
    double drawdownPct
) {}
