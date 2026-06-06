# FieldDraft — Self-Hosting Guide

## Prerequisites

- Docker + Docker Compose
- Node.js 22+ (for building the frontend)
- Rust + wasm-pack (for building the WASM kernel)

## Quick Start (Development)

```bash
# 1. Build the WASM kernel
just build-wasm

# 2. Start the frontend dev server
cd web && npm install && npm run dev

# 3. (Optional) Start the backend
cd server && go run .
```

The app runs at `http://localhost:5173` in guest/offline mode.
No backend needed for local modeling — projects save to localStorage.

## Production Deployment

### With Docker Compose

```bash
# Build and start all services
docker compose up --build -d

# Services:
#   - db:     MariaDB 11 on port 3306
#   - server: Go backend on port 8080
#   - web:    Vite frontend on port 5173
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `DB_DSN` | `fielddraft:fielddraft@tcp(db:3306)/fielddraft` | MariaDB connection string |
| `PORT` | `8080` | Server listen port |
| `SESSION_TTL` | `168h` | Session cookie lifetime (7 days) |

### Without Docker

```bash
# 1. Set up MariaDB
mysql -u root -e "CREATE DATABASE fielddraft; CREATE USER 'fielddraft'@'%' IDENTIFIED BY 'secret'; GRANT ALL ON fielddraft.* TO 'fielddraft'@'%';"

# 2. Build and run the server
cd server && go build -o fielddraft-server . && ./fielddraft-server

# 3. Build and serve the frontend (WASM kernel + Vite bundle)
just build
# `just build` runs `build-wasm` first, so the bundle always carries a
# freshly compiled kernel stamped with the web/package.json version.
# (`cd web && npm run build` alone would NOT rebuild the kernel — the
# wasm-pack output in web/src/wasm/ is a gitignored build artifact and is
# not committed.)
# Serve web/dist/ via nginx/caddy with /api proxied to :8080
```

## API Endpoints

| Method | Path | Auth | Description |
|---|---|---|---|
| POST | `/api/auth/register` | — | Create account |
| POST | `/api/auth/login` | — | Login (sets session cookie) |
| POST | `/api/auth/logout` | ✓ | Logout (clears cookie) |
| GET | `/api/me` | ✓ | Current user |
| GET | `/api/projects` | ✓ | List own projects |
| POST | `/api/projects` | ✓ | Create project |
| GET | `/api/projects/:id` | ✓ | Get project with document |
| PATCH | `/api/projects/:id` | ✓ | Update name/document |
| DELETE | `/api/projects/:id` | ✓ | Delete project |

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────┐
│  Browser     │────▶│  Go Server   │────▶│ MariaDB  │
│  (Vite/WASM) │     │  (:8080)     │     │ (:3306)  │
└─────────────┘     └──────────────┘     └──────────┘
       │
       │ WASM kernel runs entirely in the browser.
       │ The server stores projects as JSON blobs.
       │ No geometry computation on the server.
```

## Security Notes

- Passwords hashed with Argon2id (16-byte salt, 64 MB memory, 4 threads)
- Session cookies: HttpOnly, SameSite=Lax, 7-day expiry
- Per-IP rate limiting: 60 burst, 1 req/sec refill
- All project data is owner-scoped — no cross-user access
