SHELL := /bin/bash

PROJECT_NAME := $(shell sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | head -1 | tr -d '[:space:]')
PROJECT_VERSION := $(shell sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | sed -n '2p' | tr -d '[:space:]')
ifeq ($(PROJECT_NAME),)
    $(error Error: PROJECT file not found or invalid)
endif

TOP_DIR := $(CURDIR)
CARGO := cargo
BACKEND ?= wayland
DISPLAY ?= :1
WAYLAND_DISPLAY ?= wayland-0
APP_TARGET := --bin usdview
RUN_WITH ?= nixVulkan
ARGS ?=
TYPE ?= patch
HAS_REL := $(shell command -v git-rel 2>/dev/null)
RUN_ENV := WINIT_UNIX_BACKEND=$(BACKEND)
ifeq ($(BACKEND),wayland)
RUN_ENV += WAYLAND_DISPLAY=$(WAYLAND_DISPLAY)
endif
ifeq ($(BACKEND),x11)
RUN_ENV += DISPLAY=$(DISPLAY)
endif

$(info ------------------------------------------)
$(info Project: $(PROJECT_NAME) v$(PROJECT_VERSION))
$(info Display: $(BACKEND) backend)
$(info ------------------------------------------)

.PHONY: build b compile c run r serve-web build-web test t test-all check check-all harden bench clean docs release help h

build:
	@$(CARGO) build $(APP_TARGET)

b: build

compile:
	@$(CARGO) clean
	@$(MAKE) build

c: compile

run:
	@$(RUN_ENV) $(RUN_WITH) $(CARGO) run $(APP_TARGET) -- $(ARGS)

WEB_DIR := api_crates/web

serve-web:
	@cd $(WEB_DIR) && trunk serve --open

build-web:
	@cd $(WEB_DIR) && trunk build --release

r: run

test:
	@$(CARGO) test $(APP_TARGET)

t: test

test-all:
	@$(CARGO) test --workspace --all-targets

check:
	@$(CARGO) check $(APP_TARGET)

check-all:
	@$(CARGO) check --workspace --all-targets

harden:
	@git diff --check
	@$(CARGO) fmt --all -- --check
	@$(CARGO) check --workspace --no-default-features
	@$(CARGO) test --workspace --no-default-features
	@$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings
	@$(CARGO) test --workspace --all-targets --all-features

bench:
	@$(CARGO) bench

docs:
	@command -v mdbook >/dev/null 2>&1 || { echo "mdbook is not installed. Please install it first."; exit 1; }
	@mdbook build $(TOP_DIR)/book --dest-dir $(TOP_DIR)/docs
	@git add --all && git commit -m "docs: building website/mdbook"

release:
	@if [ -z "$(HAS_REL)" ]; then \
		echo "git-rel is not installed. Please install it first."; \
		exit 1; \
	fi
	@if [ -z "$(TYPE)" ]; then \
		echo "Release type not specified. Use 'make release TYPE=[patch|minor|major|m.m.p]'"; \
		exit 1; \
	fi
	@git rel $(TYPE)

clean:
	@$(CARGO) clean

help:
	@echo
	@echo "Usage: make [target]"
	@echo
	@echo "Available targets:"
	@echo "  build        Build the usdview binary"
	@echo "  compile      Clean and rebuild"
	@echo "  run          Run usdview ($(BACKEND) backend, $(RUN_WITH) wrapper)"
	@echo "  serve-web    Serve the egui_mara UI in a browser (trunk, wasm32)"
	@echo "  build-web    Build the wasm bundle to api_crates/web/dist"
	@echo "  test         Test the same app target as build/run (usdview)"
	@echo "  test-all     Run the full workspace all-target test suite"
	@echo "  check        Check the same app target as build/run (usdview)"
	@echo "  check-all    Check the full workspace all-target suite"
	@echo "  harden       Run diff whitespace check + fmt/check + strict clippy + all-feature tests"
	@echo "  bench        Run benchmarks"
	@echo "  docs         Build documentation with mdbook"
	@echo "  release      Create a new release (TYPE=patch|minor|major|m.m.p)"
	@echo "  clean        Remove Cargo build artifacts"
	@echo
	@echo "Examples:"
	@echo "  make run"
	@echo "  make run WAYLAND_DISPLAY=wayland-1 # force a specific Wayland socket"
	@echo "  make run BACKEND=x11          # force X11 / XWayland (.envrc auto-detects)"
	@echo "  make run BACKEND=wayland      # force native Wayland"
	@echo "  make run DISPLAY=:0           # target a different X server (BACKEND=x11)"
	@echo "  make run RUN_WITH=nixGL       # OpenGL wrapper instead of Vulkan"
	@echo "  make run RUN_WITH=            # no wrapper (native run)"
	@echo "  make run ARGS=\"path/to/scene.usdz\""
	@echo

h: help
