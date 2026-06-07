package com.triangulararb.dashboard.application;

import com.triangulararb.dashboard.domain.model.Trade;
import com.triangulararb.dashboard.domain.port.PnLRepository;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.*;

class PnLServiceTest {

    private PnLRepository repository;
    private PnLService service;

    @BeforeEach
    void setUp() {
        repository = new InMemRepo();
        service = new PnLService(repository);
    }

    @Test
    void emptySummary() {
        var summary = service.computeSummary();
        assertEquals(0, summary.totalTrades());
        assertEquals(0.0, summary.totalPnlUsdt());
    }

    @Test
    void recordAndSummarize() {
        var t1 = new Trade("t1", Instant.now(), "USDT", "BTC", "ETH", 5.0, 4.2, 4.2);
        var t2 = new Trade("t2", Instant.now(), "USDT", "SOL", "ETH", 3.0, 2.8, 2.8);
        var t3 = new Trade("t3", Instant.now(), "USDT", "BTC", "SOL", -1.0, -0.5, -0.5);

        service.recordTrade(t1);
        service.recordTrade(t2);
        service.recordTrade(t3);

        var summary = service.computeSummary();
        assertEquals(3, summary.totalTrades());
        assertEquals(6.5, summary.totalPnlUsdt());
        assertEquals(2, summary.winCount());
        assertEquals(1, summary.lossCount());
        assertTrue(summary.winRatePct() > 66);
        assertTrue(summary.sharpeRatio() > 0);
    }

    @Test
    void equityCurveBuilds() {
        var now = Instant.now();
        for (int i = 0; i < 5; i++) {
            service.recordTrade(new Trade(
                    "t" + i, now.plusSeconds(i), "USDT", "BTC", "ETH",
                    2.0, 1.5 + i * 0.5, 1.5 + i * 0.5
            ));
        }
        var summary = service.computeSummary();
        assertEquals(5, summary.equityCurve().size());
        assertTrue(summary.equityCurve().getLast().equity() > 10_000);
    }

    @Test
    void recentTradesReturnsLastN() {
        for (int i = 0; i < 10; i++) {
            service.recordTrade(new Trade(
                    "t" + i, Instant.now(), "A", "B", "C", 1.0, 1.0, 1.0
            ));
        }
        var recent = service.recentTrades(3);
        assertEquals(3, recent.size());
        assertEquals("t7", recent.getFirst().id());
    }

    private static class InMemRepo implements PnLRepository {
        private final List<Trade> trades = Collections.synchronizedList(new ArrayList<>());

        @Override
        public void save(Trade trade) { trades.add(trade); }

        @Override
        public List<Trade> findAll() { return List.copyOf(trades); }

        @Override
        public Optional<Trade> findById(String id) {
            return trades.stream().filter(t -> t.id().equals(id)).findFirst();
        }

        @Override
        public void clear() { trades.clear(); }
    }
}
