FROM node:22.12-alpine AS builder

WORKDIR /builder

# Optionally specify a specific Komodo host.
ARG VITE_KOMODO_HOST=""
ENV VITE_KOMODO_HOST=$VITE_KOMODO_HOST

COPY ./client/core/ts/package.json ./client/core/ts/yarn.lock ./client/
RUN cd client && yarn --frozen-lockfile

COPY ./ui/package.json ./ui/yarn.lock ./ui/
RUN cd ui && yarn --frozen-lockfile

COPY ./client/core/ts ./client
RUN cd client && yarn build && yarn link

COPY ./ui ./ui
RUN cd ui && yarn link komodo_client && yarn build

# Copy just the static ui to scratch image
FROM scratch

COPY --from=builder /builder/ui/dist /ui

LABEL org.opencontainers.image.source="https://github.com/intezya/komodo"
LABEL org.opencontainers.image.description="Komodo UI"
LABEL org.opencontainers.image.licenses="GPL-3.0"
