package com.triangulararb.dashboard.infrastructure.redis;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.triangulararb.dashboard.domain.model.Trade;
import com.triangulararb.dashboard.domain.port.TradeStreamListener;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.data.redis.connection.Message;
import org.springframework.data.redis.connection.MessageListener;
import org.springframework.stereotype.Component;

import java.io.IOException;

@Component
public class RedisTradeSubscriber implements MessageListener {

    private static final Logger log = LoggerFactory.getLogger(RedisTradeSubscriber.class);

    private final ObjectMapper mapper;
    private final TradeStreamListener listener;

    public RedisTradeSubscriber(ObjectMapper mapper, TradeStreamListener listener) {
        this.mapper = mapper;
        this.listener = listener;
    }

    @Override
    public void onMessage(Message message, byte[] pattern) {
        try {
            var trade = mapper.readValue(message.getBody(), Trade.class);
            listener.onTrade(trade);
        } catch (IOException e) {
            log.error("Failed to deserialize trade from Redis: {}", e.getMessage());
        }
    }
}
