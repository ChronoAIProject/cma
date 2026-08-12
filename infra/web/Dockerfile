# syntax=docker/dockerfile:1
FROM node:26-alpine AS build
WORKDIR /build
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.29-alpine AS runtime
USER 101:101
COPY --from=build /build/dist /usr/share/nginx/html
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s CMD ["/usr/bin/curl", "-fsS", "http://127.0.0.1:8080/"]
