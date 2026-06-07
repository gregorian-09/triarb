package com.triangulararb.dashboard.application;

import com.triangulararb.dashboard.domain.model.EquityPoint;
import com.triangulararb.dashboard.domain.model.PnLSummary;
import com.triangulararb.dashboard.domain.model.Trade;
import com.triangulararb.dashboard.domain.port.PnLRepository;
import org.springframework.stereotype.Service;

import java.util.ArrayList;
import java.util.List;

@Service
public class PnLService {

    private final PnLRepository repository;

    public PnLService(PnLRepository repository) {
        this.repository = repository;
    }

    public void recordTrade(Trade trade) {
        repository.save(trade);
    }

    public PnLSummary computeSummary() {
        var trades = repository.findAll();
        if (trades.isEmpty()) {
            return new PnLSummary(0, 0, 0, 0, 0, 0, 0, 0, List.of());
        }

        var wins = trades.stream().filter(t -> t.pnlUsdt() > 0).toList();
        var losses = trades.stream().filter(t -> t.pnlUsdt() <= 0).toList();
        double totalPnl = trades.stream().mapToDouble(Trade::pnlUsdt).sum();
        double avgProfit = trades.stream().mapToDouble(Trade::actualProfitBps).average().orElse(0);
        double winRate = (double) wins.size() / trades.size() * 100;
        double maxDrawdown = computeMaxDrawdown(trades);
        double sharpe = computeSharpe(trades);

        var equityCurve = buildEquityCurve(trades);

        return new PnLSummary(
            Math.round(totalPnl * 100.0) / 100.0,
            trades.size(),
            wins.size(),
            losses.size(),
            Math.round(winRate * 100.0) / 100.0,
            Math.round(sharpe * 10000.0) / 10000.0,
            Math.round(maxDrawdown * 100.0) / 100.0,
            Math.round(avgProfit * 100.0) / 100.0,
            equityCurve
        );
    }

    public List<Trade> recentTrades(int limit) {
        var all = repository.findAll();
        int from = Math.max(0, all.size() - limit);
        return all.subList(from, all.size());
    }

    private List<EquityPoint> buildEquityCurve(List<Trade> trades) {
        double running = 10_000.0;
        double peak = running;
        var points = new ArrayList<EquityPoint>(trades.size());
        for (var t : trades) {
            running += t.pnlUsdt();
            peak = Math.max(peak, running);
            double dd = (running - peak) / peak * 100;
            points.add(new EquityPoint(t.timestamp(), running, dd));
        }
        return points;
    }

    private double computeMaxDrawdown(List<Trade> trades) {
        double running = 10_000.0;
        double peak = running;
        double maxDd = 0;
        for (var t : trades) {
            running += t.pnlUsdt();
            peak = Math.max(peak, running);
            double dd = (running - peak) / peak;
            maxDd = Math.min(maxDd, dd);
        }
        return maxDd;
    }

    private double computeSharpe(List<Trade> trades) {
        if (trades.size() < 2) return 0;
        double[] returns = new double[trades.size()];
        double running = 10_000.0;
        for (int i = 0; i < trades.size(); i++) {
            double prev = running;
            running += trades.get(i).pnlUsdt();
            returns[i] = (running - prev) / prev;
        }
        double mean = 0;
        for (double r : returns) mean += r;
        mean /= returns.length;
        double variance = 0;
        for (double r : returns) variance += (r - mean) * (r - mean);
        variance /= returns.length;
        if (variance == 0) return 0;
        double std = Math.sqrt(variance);
        return Math.sqrt(returns.length) * mean / std;
    }
}
