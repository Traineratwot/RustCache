BIN      := target/release/rustcache
PID_FILE := rustcache.pid
CONFIG   ?= config.toml
LOG      ?= rustcache.log

.PHONY: build start stop dev test

# Полная сборка: UI (bun) → web/dist, затем релизный бинарник с embed-ui
build:
	cd web && bun install
	cd web && bun run build
	cargo build --release --features embed-ui

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
