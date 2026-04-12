FROM golang:1.26-alpine AS build
WORKDIR /app
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /fielddraft-server .

FROM alpine:3
COPY --from=build /fielddraft-server /usr/local/bin/fielddraft-server
EXPOSE 8080
CMD ["fielddraft-server"]
