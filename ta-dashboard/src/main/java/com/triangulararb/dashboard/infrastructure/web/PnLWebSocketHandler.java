package com.triangulararb.dashboard.infrastructure.web;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.triangulararb.dashboard.domain.model.Trade;
import com.triangulararb.dashboard.domain.port.TradeStreamListener;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.stereotype.Component;
import org.springframework.web.socket.CloseStatus;
import org.springframework.web.socket.TextMessage;
import org.springframework.web.socket.WebSocketSession;
import org.springframework.web.socket.handler.TextWebSocketHandler;

import java.io.IOException;
import java.util.Set;
import java.util.concurrent.CopyOnWriteArraySet;

@Component
public class PnLWebSocketHandler extends TextWebSocketHandler implements TradeStreamListener {

    private static final Logger log = LoggerFactory.getLogger(PnLWebSocketHandler.class);

    private final Set<WebSocketSession> sessions = new CopyOnWriteArraySet<>();
    private final ObjectMapper mapper;

    public PnLWebSocketHandler(ObjectMapper mapper) {
        this.mapper = mapper;
    }

    @Override
    public void afterConnectionEstablished(WebSocketSession session) {
        sessions.add(session);
        log.info("WebSocket client connected: {}", session.getId());
    }

    @Override
    public void afterConnectionClosed(WebSocketSession session, CloseStatus status) {
        sessions.remove(session);
        log.info("WebSocket client disconnected: {}", session.getId());
    }

    @Override
    public void onTrade(Trade trade) {
        try {
            var json = mapper.writeValueAsString(trade);
            var msg = new TextMessage(json);
            for (var session : sessions) {
                if (session.isOpen()) {
                    try {
                        session.sendMessage(msg);
                    } catch (IOException e) {
                        log.warn("Failed to send WS message to {}: {}", session.getId(), e.getMessage());
                    }
                }
            }
        } catch (IOException e) {
            log.error("Failed to serialize trade for WS broadcast: {}", e.getMessage());
        }
    }
}
