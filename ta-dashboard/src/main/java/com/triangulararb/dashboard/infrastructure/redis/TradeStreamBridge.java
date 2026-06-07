package com.triangulararb.dashboard.infrastructure.redis;

import com.triangulararb.dashboard.domain.port.TradeStreamListener;
import com.triangulararb.dashboard.application.PnLService;
import com.triangulararb.dashboard.domain.model.Trade;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.stereotype.Component;

@Component
public class TradeStreamBridge implements TradeStreamListener {

    private static final Logger log = LoggerFactory.getLogger(TradeStreamBridge.class);

    private final PnLService pnlService;

    public TradeStreamBridge(PnLService pnlService) {
        this.pnlService = pnlService;
    }

    @Override
    public void onTrade(Trade trade) {
        log.info("Received trade: {} profit={}bps pnl={}USDT", trade.id(), trade.actualProfitBps(), trade.pnlUsdt());
        pnlService.recordTrade(trade);
    }
}
