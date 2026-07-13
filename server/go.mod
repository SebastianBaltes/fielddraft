module github.com/sorokan/fielddraft/server

go 1.26.1

require (
	github.com/go-sql-driver/mysql v1.9.3
	golang.org/x/crypto v0.50.0
)

require (
	filippo.io/edwards25519 v1.1.0 // indirect
	golang.org/x/sys v0.43.0 // indirect
)

require github.com/SebastianBaltes/BaseAIChat/server v0.1.0

replace github.com/SebastianBaltes/BaseAIChat/server => ../third_party/baseaichat/server
