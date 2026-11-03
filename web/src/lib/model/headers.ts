/** Common request header names, suggested in the headers table. */
export const HEADER_NAMES = [
  "Accept",
  "Accept-Encoding",
  "Accept-Language",
  "Authorization",
  "Cache-Control",
  "Connection",
  "Content-Encoding",
  "Content-Type",
  "Cookie",
  "If-Match",
  "If-Modified-Since",
  "If-None-Match",
  "Origin",
  "Prefer",
  "Range",
  "Referer",
  "User-Agent",
  "X-API-Key",
  "X-Request-ID",
  "X-Forwarded-For",
] as const;

/** Common content types, suggested for text and file bodies. */
export const CONTENT_TYPES = [
  "application/json",
  "application/xml",
  "application/x-www-form-urlencoded",
  "application/octet-stream",
  "text/plain; charset=utf-8",
  "text/csv",
  "text/html",
  "text/xml",
] as const;
