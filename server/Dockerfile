# Build context is the REPO ROOT, not server/ (ticket #347): the Go module
# `replace`s github.com/SebastianBaltes/BaseAIChat/server to the submodule under
# third_party/baseaichat, and a path outside the build context does not exist
# inside the builder. Both compose files pass `context: .` accordingly.
FROM golang:1.26-alpine AS build
WORKDIR /src
# The chat-proxy source must land BEFORE `go mod download`: the replace directive
# points at it, so the module graph cannot resolve without it.
COPY third_party/baseaichat/server ./third_party/baseaichat/server
COPY server/go.mod server/go.sum ./server/
WORKDIR /src/server
RUN go mod download
COPY server .
RUN CGO_ENABLED=0 go build -o /fielddraft-server .

FROM alpine:3
COPY --from=build /fielddraft-server /usr/local/bin/fielddraft-server
EXPOSE 8080
CMD ["fielddraft-server"]
