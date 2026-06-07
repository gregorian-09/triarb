package com.triangulararb.dashboard.domain.port;

import com.triangulararb.dashboard.domain.model.Trade;
import java.util.List;
import java.util.Optional;

public interface PnLRepository {
    void save(Trade trade);
    List<Trade> findAll();
    Optional<Trade> findById(String id);
    void clear();
}
