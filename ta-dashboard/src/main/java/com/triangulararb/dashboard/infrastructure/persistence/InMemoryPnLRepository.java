package com.triangulararb.dashboard.infrastructure.persistence;

import com.triangulararb.dashboard.domain.model.Trade;
import com.triangulararb.dashboard.domain.port.PnLRepository;
import org.springframework.stereotype.Repository;

import java.util.*;
import java.util.concurrent.CopyOnWriteArrayList;

@Repository
public class InMemoryPnLRepository implements PnLRepository {

    private final List<Trade> trades = new CopyOnWriteArrayList<>();

    @Override
    public void save(Trade trade) {
        trades.add(trade);
    }

    @Override
    public List<Trade> findAll() {
        return Collections.unmodifiableList(trades);
    }

    @Override
    public Optional<Trade> findById(String id) {
        return trades.stream().filter(t -> t.id().equals(id)).findFirst();
    }

    @Override
    public void clear() {
        trades.clear();
    }
}
