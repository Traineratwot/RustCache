BIN         := target/release/rustcache
CLIENT_BIN  := target/release/rustcache-client
PID_FILE    := rustcache.pid
CLIENT_PID  := rustcache-client.pid
CONFIG      ?= config.toml
LOG         ?= rustcache.log
CLIENT_LOG  ?= rustcache-client.log
CLIENT_MODE ?= off

.PHONY: build build-client build-all start stop start-client stop-client dev test clean-pids

# Полная сборка сервера: UI (bun) → web/dist, затем релизный бинарник с embed-ui
build:
	cd web && bun install
	cd web && bun run build
	cargo build --release --features embed-ui

# Клиент: CLI + GUI (eframe + tray) + TUN capture
build-client:
	cargo build -p rustcache-client --release --features gui,tun

# Всё сразу
build-all: build build-client

# Запуск прокси в фоне (PID → rustcache.pid, лог → rustcache.log)
start: $(BIN)
	$(BIN) run --config $(CONFIG) >$(LOG) 2>&1 & echo $$! > $(PID_FILE)
	@echo "rustcache started, pid=$$(cat $(PID_FILE)), log=$(LOG)"

# Остановка по PID-файлу
stop:
	@if [ -f $(PID_FILE) ]; then \
		kill $$(cat $(PID_FILE)) && rm -f $(PID_FILE) && echo "stopped"; \
	else \
		echo "no $(PID_FILE) — not running?"; \
	fi

# --- client ---

# Запуск клиента (GUI + локальный proxy). CLIENT_MODE=off|system
# setsid — чтобы процесс пережил закрытие shell (иначе падает вместе с make).
start-client: $(CLIENT_BIN)
	@setsid $(CLIENT_BIN) gui --mode $(CLIENT_MODE) >$(CLIENT_LOG) 2>&1 < /dev/null & \
		echo $$! > $(CLIENT_PID)
	@sleep 0.4
	@if kill -0 $$(cat $(CLIENT_PID)) 2>/dev/null; then \
		echo "rustcache-client started, pid=$$(cat $(CLIENT_PID)), mode=$(CLIENT_MODE), log=$(CLIENT_LOG)"; \
	else \
		echo "failed to start:"; cat $(CLIENT_LOG); exit 1; \
	fi

# Остановка клиента + восстановление system proxy (dirty-restore при следующем запуске)
stop-client:
	@if [ -f $(CLIENT_PID) ]; then \
		kill $$(cat $(CLIENT_PID)) 2>/dev/null; \
		for i in 1 2 3 4 5; do kill -0 $$(cat $(CLIENT_PID)) 2>/dev/null || break; sleep 0.2; done; \
		kill -9 $$(cat $(CLIENT_PID)) 2>/dev/null || true; \
		rm -f $(CLIENT_PID); \
		echo "rustcache-client stopped"; \
	else \
		pkill -f 'rustcache-client gui' 2>/dev/null && echo "rustcache-client stopped (pkill)" \
			|| echo "no $(CLIENT_PID) — not running?"; \
	fi

clean-pids:
	rm -f $(PID_FILE) $(CLIENT_PID)

# Разработка: backend + Vite; Ctrl+C гасит оба
dev:
	@trap 'kill 0' EXIT INT TERM; \
	 cargo run -p rustcache -- run --config $(CONFIG) & \
	 cd web && bun run dev

# Все тесты workspace
test:
	cargo test --workspace

$(BIN):
	cargo build --release --features embed-ui

$(CLIENT_BIN):
	cargo build -p rustcache-client --release --features gui,tun
