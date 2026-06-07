package com.triangulararb.dashboard.domain.port;

import com.triangulararb.dashboard.domain.model.Trade;

@FunctionalInterface
public interface TradeStreamListener {
    void onTrade(Trade trade);
}
