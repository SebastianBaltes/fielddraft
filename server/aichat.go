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
	"log"
	"net/http"
	"os"
	"strings"

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

// newAIChatProxy builds the proxy, or nil when no API key is configured.
func (s *Server) newAIChatProxy() http.Handler {
	key := strings.TrimSpace(os.Getenv("OPENROUTER_API_KEY"))
	if key == "" {
		log.Print("ai chat: OPENROUTER_API_KEY unset — chat disabled")
		return nil
	}
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
		// The proxy spends real money on every call, so it is session-gated like
		// any other write route — an unauthenticated caller never reaches upstream.
		Authorize: func(r *http.Request) error {
			if _, user := s.requireAuth(r); user == nil {
				return aichat.ErrUnauthorized
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
