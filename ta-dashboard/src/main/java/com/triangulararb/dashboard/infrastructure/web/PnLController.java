package com.triangulararb.dashboard.infrastructure.web;

import com.triangulararb.dashboard.application.PnLService;
import com.triangulararb.dashboard.domain.model.PnLSummary;
import com.triangulararb.dashboard.domain.model.Trade;
import org.springframework.http.MediaType;
import org.springframework.web.bind.annotation.*;

import java.util.List;

@RestController
@RequestMapping("/api")
public class PnLController {

    private final PnLService pnlService;

    public PnLController(PnLService pnlService) {
        this.pnlService = pnlService;
    }

    @GetMapping(value = "/pnl/summary", produces = MediaType.APPLICATION_JSON_VALUE)
    public PnLSummary summary() {
        return pnlService.computeSummary();
    }

    @GetMapping(value = "/trades", produces = MediaType.APPLICATION_JSON_VALUE)
    public List<Trade> recentTrades(
            @RequestParam(name = "limit", defaultValue = "100") int limit) {
        return pnlService.recentTrades(limit);
    }

    @GetMapping(value = "/trades/{id}", produces = MediaType.APPLICATION_JSON_VALUE)
    public Trade tradeById(@PathVariable String id) {
        return pnlService.recentTrades(1000).stream()
                .filter(t -> t.id().equals(id))
                .findFirst()
                .orElse(null);
    }
}
