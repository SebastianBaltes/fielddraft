// AI-chat proxy (ticket #347).
//
// FieldDraft embeds BaseAIChat — a reusable chat agent whose AGENT LOOP RUNS IN
// THE BROWSER: the model writes JavaScript against the app's own API and drives
// the kernel/store directly, where the state lives. The Go side is deliberately
// dumb: a pass-through to the model provider that holds the API key, checks the
// session, pins the model allowlist and rate-limits. No state, no queue.
//
// Mounted at /aichat/ (the browser's chat client sets the upstream path in
// X-Target-Path; the proxy refuses anything with ".." or an absolute URL).
//
// The provider is OpenRouter. Without OPENROUTER_API_KEY the feature is simply
// OFF: the routes are not registered and the web app hides the chat — the same
// convention as the Onshape proxy and the mesher. It never falls back to an
// unauthenticated upstream.
package main

import (
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	aichat "github.com/SebastianBaltes/BaseAIChat/server"
)

// aichatModels is the model allowlist. The request comes FROM THE BROWSER, so
// without this list a client could pick the most expensive model on OpenRouter.
// Explicit ids, no wildcards (a wildcard drags the premium tiers in with it).
//
// The vision model MUST be listed too — as soon as a screenshot is attached the
// client switches to it, and an unlisted model turns the whole turn into a 403.
//
// Override per deployment with AICHAT_MODELS (comma-separated). The FIRST entry
// is the model the chat actually talks to — the client takes models[0] from
// /api/aichat/config (web/src/aichat/setup.ts), so the ORDER is the setting.
//
// To see what OpenRouter offers, ask the proxy itself: since baseaichat v0.3.1
// the allowlist gates POST (inference) only, GET is metadata and passes through.
//
//	curl -H 'X-Target-Path: models' http://localhost:8080/aichat/models
var aichatDefaultModels = []string{
	"google/gemini-3-flash-preview", // chat + tools + vision (verified via the proxy)
	"anthropic/claude-haiku-4.5",
	"anthropic/claude-sonnet-4.5",
}

func aichatAllowedModels() []string {
	if raw := strings.TrimSpace(os.Getenv("AICHAT_MODELS")); raw != "" {
		var out []string
		for _, m := range strings.Split(raw, ",") {
			if m = strings.TrimSpace(m); m != "" {
				out = append(out, m)
			}
		}
		if len(out) > 0 {
			return out
		}
	}
	return aichatDefaultModels
}

// aichatAPIKey reads the OpenRouter key — preferring a FILE over an env var.
//
// OPENROUTER_API_KEY_FILE points at a file (a Docker/Swarm secret, a
// root-owned 0600 file next to the compose stack). That is strictly better than
// the env var: an env var is visible in `docker inspect`, in `/proc/<pid>/environ`,
// and it is inherited by every child process the container spawns. A file is
// readable by exactly whoever the filesystem lets read it, and it never shows up
// in a process listing or a crash dump of the environment.
//
// OPENROUTER_API_KEY stays supported (dev convenience, and it is what the .env
// carries) — but a deployment that means it should use the file.
func aichatAPIKey() string {
	if path := strings.TrimSpace(os.Getenv("OPENROUTER_API_KEY_FILE")); path != "" {
		data, err := os.ReadFile(path)
		if err != nil {
			log.Printf("ai chat: OPENROUTER_API_KEY_FILE %s: %v", path, err)
			return ""
		}
		return strings.TrimSpace(string(data))
	}
	return strings.TrimSpace(os.Getenv("OPENROUTER_API_KEY"))
}

// aichatDailyQuota is the per-user cap on chat requests per day. The minute-level
// rate limit stops a burst; it does NOT stop a patient script from spending the
// whole budget over an afternoon, and the account it needs is one invite away.
// 0 disables the cap (AICHAT_DAILY_QUOTA).
func aichatDailyQuota() int {
	if raw := strings.TrimSpace(os.Getenv("AICHAT_DAILY_QUOTA")); raw != "" {
		if n, err := strconv.Atoi(raw); err == nil && n >= 0 {
			return n
		}
	}
	return 300
}

// dailyQuota counts requests per user and calendar day (UTC).
type dailyQuota struct {
	mu    sync.Mutex
	limit int
	day   string
	used  map[string]int
}

func newDailyQuota(limit int) *dailyQuota {
	return &dailyQuota{limit: limit, used: map[string]int{}}
}

// allow records one request for user and reports whether it stays within budget.
func (q *dailyQuota) allow(user string) bool {
	if q.limit <= 0 || user == "" {
		return true
	}
	q.mu.Lock()
	defer q.mu.Unlock()
	today := time.Now().UTC().Format("2006-01-02")
	if q.day != today {
		q.day = today
		q.used = map[string]int{}
	}
	if q.used[user] >= q.limit {
		return false
	}
	q.used[user]++
	return true
}

// crossSiteBlocked rejects a POST that a FOREIGN page made the browser send.
//
// The naive check — "Origin host must equal r.Host" — is wrong behind a proxy:
// the browser's Origin is the PUBLIC host, while r.Host is whatever the proxy
// dialled (localhost:8080 in dev behind Vite, the container name in prod behind
// Caddy). Comparing them rejects every legitimate request; it did exactly that
// when tried against the running server.
//
// What actually holds, and needs no knowledge of our own public name:
//
//   - The session cookie is SameSite=Lax, so a cross-site POST carries NO
//     session at all → requireAuth already rejects it.
//   - The chat client always sends the custom header X-Target-Path. A cross-site
//     fetch cannot set a custom header without a CORS PREFLIGHT, and this server
//     answers no preflight — so the browser never sends the actual request. A
//     simple form POST (the classic CSRF vector, which needs no preflight) cannot
//     set headers at all and is therefore rejected here.
//
// AICHAT_ALLOWED_ORIGINS (comma-separated) additionally pins the Origin when a
// deployment wants to state its public name explicitly.
func crossSiteBlocked(r *http.Request) bool {
	if r.Method != http.MethodPost {
		return false
	}
	if strings.TrimSpace(r.Header.Get("X-Target-Path")) == "" {
		return true
	}
	allowed := strings.TrimSpace(os.Getenv("AICHAT_ALLOWED_ORIGINS"))
	if allowed == "" {
		return false
	}
	origin := strings.TrimSpace(r.Header.Get("Origin"))
	if origin == "" {
		return false // same-origin clients may omit it; the header check stands
	}
	for _, a := range strings.Split(allowed, ",") {
		if strings.EqualFold(strings.TrimSpace(a), origin) {
			return false
		}
	}
	return true
}

// newAIChatProxy builds the proxy, or nil when no API key is configured.
func (s *Server) newAIChatProxy() http.Handler {
	key := aichatAPIKey()
	if key == "" {
		log.Print("ai chat: no API key (OPENROUTER_API_KEY[_FILE]) — chat disabled")
		return nil
	}
	quota := newDailyQuota(aichatDailyQuota())
	models := aichatAllowedModels()
	proxy, err := aichat.NewProxy(aichat.Config{
		Provider:      "openrouter",
		APIKey:        key,
		AllowedModels: models,
		// Per USER, not per IP: several people behind one NAT must not share a
		// budget, and a logged-in user is the unit we can actually hold to it.
		RateLimit: 120,
		ClientKey: func(r *http.Request) string {
			if _, user := s.requireAuth(r); user != nil {
				return user.ID
			}
			return ""
		},
		// The proxy spends real money on every call, so THREE things must hold
		// before a request reaches OpenRouter — this hook is the only place they
		// all meet:
		//
		//   1. A valid session. An anonymous caller never reaches upstream, and a
		//      banned user has no session left (auth.go deletes them on ban).
		//   2. Not a cross-site POST (crossSiteBlocked) — defence in depth behind
		//      the SameSite=Lax cookie.
		//   3. The user's daily budget. The minute rate-limit stops a burst; this
		//      stops a patient one.
		Authorize: func(r *http.Request) error {
			_, user := s.requireAuth(r)
			if user == nil {
				return aichat.ErrUnauthorized
			}
			if crossSiteBlocked(r) {
				return errors.New("cross-site request")
			}
			if r.Method == http.MethodPost && !quota.allow(user.ID) {
				return errors.New("daily assistant quota exhausted")
			}
			return nil
		},
	})
	if err != nil {
		log.Printf("ai chat: proxy init failed: %v", err)
		return nil
	}
	log.Printf("ai chat: openrouter proxy up (models: %s)", strings.Join(models, ", "))
	return http.StripPrefix("/aichat", proxy)
}

// handleAIChatConfig tells the web app whether the chat is available and which
// models it may ask for — so the client never has to hardcode a model id that
// the proxy would then reject.
func (s *Server) handleAIChatConfig(w http.ResponseWriter, r *http.Request) {
	if _, user := s.requireAuth(r); user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	json.NewEncoder(w).Encode(map[string]any{
		"enabled": s.aichat != nil,
		"models":  aichatAllowedModels(),
	})
}
