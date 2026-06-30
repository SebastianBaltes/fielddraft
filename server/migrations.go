// Minimal embedded migration runner.
//
// Numbered *.sql files under migrations/ are embedded into the binary and
// applied in lexical order on startup. A schema_migrations table tracks which
// versions are applied; pending ones run inside their own transaction. No
// heavyweight migration library — just enough to bring the schema up.

package main

import (
	"database/sql"
	"embed"
	"fmt"
	"io/fs"
	"log"
	"sort"
	"strconv"
	"strings"
)

//go:embed migrations/*.sql
var migrationFS embed.FS

type migration struct {
	version int
	name    string
	sql     string
}

// loadMigrations reads and parses every embedded migration file. A filename
// must start with a zero-padded integer version, e.g. "0001_init.sql".
func loadMigrations() ([]migration, error) {
	entries, err := fs.ReadDir(migrationFS, "migrations")
	if err != nil {
		return nil, err
	}
	var out []migration
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".sql") {
			continue
		}
		prefix := e.Name()
		if i := strings.IndexByte(prefix, '_'); i >= 0 {
			prefix = prefix[:i]
		} else {
			prefix = strings.TrimSuffix(prefix, ".sql")
		}
		ver, err := strconv.Atoi(prefix)
		if err != nil {
			return nil, fmt.Errorf("migration %q: bad version prefix: %w", e.Name(), err)
		}
		body, err := migrationFS.ReadFile("migrations/" + e.Name())
		if err != nil {
			return nil, err
		}
		out = append(out, migration{version: ver, name: e.Name(), sql: string(body)})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].version < out[j].version })
	return out, nil
}

// runMigrations applies all pending migrations in order.
func runMigrations(db *sql.DB) error {
	if _, err := db.Exec(`CREATE TABLE IF NOT EXISTS schema_migrations (
		version INT NOT NULL PRIMARY KEY,
		name    VARCHAR(255) NOT NULL,
		applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
	) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4`); err != nil {
		return fmt.Errorf("create schema_migrations: %w", err)
	}

	applied := map[int]bool{}
	rows, err := db.Query("SELECT version FROM schema_migrations")
	if err != nil {
		return fmt.Errorf("read schema_migrations: %w", err)
	}
	for rows.Next() {
		var v int
		if err := rows.Scan(&v); err != nil {
			rows.Close()
			return err
		}
		applied[v] = true
	}
	rows.Close()

	migs, err := loadMigrations()
	if err != nil {
		return err
	}

	for _, m := range migs {
		if applied[m.version] {
			continue
		}
		log.Printf("migration: applying %s", m.name)
		tx, err := db.Begin()
		if err != nil {
			return err
		}
		for _, stmt := range splitSQL(m.sql) {
			if _, err := tx.Exec(stmt); err != nil {
				tx.Rollback()
				return fmt.Errorf("migration %s: %w", m.name, err)
			}
		}
		if _, err := tx.Exec("INSERT INTO schema_migrations (version, name) VALUES (?, ?)", m.version, m.name); err != nil {
			tx.Rollback()
			return fmt.Errorf("migration %s record: %w", m.name, err)
		}
		if err := tx.Commit(); err != nil {
			return fmt.Errorf("migration %s commit: %w", m.name, err)
		}
	}
	return nil
}

// splitSQL breaks a migration file into individual statements on ';' at the
// end of a line. The MySQL driver executes one statement per Exec, so we must
// split. Line comments (-- …) are stripped; we have no string-literal or
// stored-procedure DDL in these files, so this simple splitter is sufficient.
func splitSQL(src string) []string {
	var stmts []string
	var cur strings.Builder
	for _, line := range strings.Split(src, "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "--") || trimmed == "" {
			continue
		}
		cur.WriteString(line)
		cur.WriteByte('\n')
		if strings.HasSuffix(trimmed, ";") {
			s := strings.TrimSpace(cur.String())
			s = strings.TrimSuffix(s, ";")
			if s != "" {
				stmts = append(stmts, s)
			}
			cur.Reset()
		}
	}
	if rest := strings.TrimSpace(cur.String()); rest != "" {
		stmts = append(stmts, rest)
	}
	return stmts
}
