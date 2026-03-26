# ─────────────────────────────────────────────────────────────
# 🦈 treeshark — Makefile
# ─────────────────────────────────────────────────────────────

BINARY     := treeshark
CARGO      := cargo
INSTALL_DIR := $(HOME)/.cargo/bin
RELEASE_BIN := target/release/$(BINARY)
CONFIG_SRC  := config.yml
CONFIG_DST  := $(HOME)/.config/treeshark/config.yml
DB_FILE     := treeshark.db

.PHONY: help setup deps build release install config uninstall clean purge run-scan run-list run-delete lint fmt test check

# ─────────────────────── Default ────────────────────────────

help: ## Show this help
	@echo ""
	@echo "  🦈 treeshark — Makefile targets"
	@echo "  ────────────────────────────────────────"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'
	@echo ""

# ─────────────────────── Setup ──────────────────────────────

setup: deps build install config ## Full setup: deps + build + install + config
	@echo ""
	@echo "  ✅ treeshark installed to $(INSTALL_DIR)/$(BINARY)"
	@echo "  ✅ Config at $(CONFIG_DST)"
	@echo ""
	@echo "  Run:  treeshark --help"
	@echo ""

deps: ## Install Rust toolchain if missing
	@command -v rustc >/dev/null 2>&1 || { \
		echo "  📦 Installing Rust via rustup..."; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; \
		echo "  ✅ Rust installed. Restart your shell or run: source $$HOME/.cargo/env"; \
	}
	@command -v cargo >/dev/null 2>&1 && echo "  ✅ Rust $$(rustc --version) ready" || true

build: ## Build release binary
	@echo "  🔨 Building release binary..."
	$(CARGO) build --release
	@echo "  ✅ Built: $(RELEASE_BIN)"
	@ls -lh $(RELEASE_BIN) | awk '{print "  📦 Size:", $$5}'

release: build ## Alias for build

install: build ## Install binary to ~/.cargo/bin
	@mkdir -p $(INSTALL_DIR)
	@cp $(RELEASE_BIN) $(INSTALL_DIR)/$(BINARY)
	@chmod +x $(INSTALL_DIR)/$(BINARY)
	@echo "  ✅ Installed to $(INSTALL_DIR)/$(BINARY)"

config: ## Copy default config to ~/.config/treeshark/
	@if [ ! -f "$(CONFIG_DST)" ]; then \
		mkdir -p $$(dirname $(CONFIG_DST)); \
		cp $(CONFIG_SRC) $(CONFIG_DST); \
		echo "  ✅ Config written to $(CONFIG_DST)"; \
	else \
		echo "  ⏭️  Config already exists at $(CONFIG_DST) — skipping"; \
	fi

uninstall: ## Remove installed binary and config
	@rm -f $(INSTALL_DIR)/$(BINARY)
	@echo "  🗑️  Removed $(INSTALL_DIR)/$(BINARY)"
	@rm -rf $(HOME)/.config/treeshark
	@echo "  🗑️  Removed $(HOME)/.config/treeshark/"

# ─────────────────────── Dev ────────────────────────────────

clean: ## Remove build artifacts and database
	$(CARGO) clean
	@rm -f $(DB_FILE)
	@echo "  🧹 Cleaned build artifacts and database"

purge: clean uninstall ## Full removal: clean + uninstall
	@echo "  🧹 Purged everything"

lint: ## Run clippy linter
	$(CARGO) clippy -- -W clippy::all

fmt: ## Format code
	$(CARGO) fmt

check: ## Check compilation without building
	$(CARGO) check

test: ## Run tests
	$(CARGO) test

# ─────────────────────── Run shortcuts ──────────────────────

run-scan: build ## Build and run scan
	$(RELEASE_BIN) scan

run-list: ## Show cached results
	$(RELEASE_BIN) list

run-delete: ## Interactive delete
	$(RELEASE_BIN) delete

run-stats: ## Show database stats
	$(RELEASE_BIN) stats

run-history: ## Show scan history
	$(RELEASE_BIN) history
