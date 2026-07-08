# ─────────────────────────────────────────────
# bitcoindata light-client – Development Container
# ─────────────────────────────────────────────
FROM --platform=$BUILDPLATFORM node:24-alpine3.23 AS base

ENV PNPM_HOME=/pnpm \
    PATH=/pnpm:$PATH \
    VITE_HOST=0.0.0.0

# Corepack is bundled with Node and provides the pnpm shim declared in
# package.json. No native build toolchain is required for the current browser UI.
RUN corepack enable

WORKDIR /app

# ─────────────────────────────────────────────
# dev stage – used by podman/docker compose
# ─────────────────────────────────────────────
FROM base AS dev

# Pre-install dependencies from manifests first so the image layer can be
# cached separately from source changes. If a pnpm lockfile is present, use it.
COPY package.json pnpm-lock.yaml* ./
RUN pnpm config set store-dir /app/node_modules/.pnpm-store \
  && if [ -f pnpm-lock.yaml ]; then pnpm install --frozen-lockfile; else pnpm install; fi

# Keep a complete copy in the image for non-bind-mounted runs. Compose bind
# mounts the source tree over /app and keeps node_modules in a named volume.
COPY . ./

# Compose publishes the selected container port; no fixed EXPOSE is needed.
CMD ["sh", "-lc", "pnpm exec vite --host \"${VITE_HOST:-0.0.0.0}\" --port \"${VITE_PORT:-5173}\" --strictPort"]
