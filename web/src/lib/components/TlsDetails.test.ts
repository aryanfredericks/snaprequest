// @vitest-environment jsdom
import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import TlsDetails from "./TlsDetails.svelte";

describe("TlsDetails", () => {
  it("shows the protocol and the certificate chain with expiry warnings", () => {
    const soon = new Date(Date.now() + 5 * 86_400_000).toISOString();
    render(TlsDetails, {
      info: {
        version: "TLSv1.3",
        cipher: "TLS13_AES_128_GCM_SHA256",
        alpn: "h2",
        peer_certificates: [
          {
            subject: "CN=api.example.test",
            issuer: "CN=Example CA",
            not_before: "2026-01-01T00:00:00Z",
            not_after: soon,
            serial: "0a:0b",
            subject_alt_names: ["api.example.test", "*.example.test"],
            sha256_fingerprint: "AA:BB",
          },
          {
            subject: "CN=Example CA",
            issuer: "CN=Example CA",
            not_before: "2020-01-01T00:00:00Z",
            not_after: "2021-01-01T00:00:00Z",
            serial: "01",
            sha256_fingerprint: "CC:DD",
          },
        ],
      },
      host: "api.example.test",
    });
    expect(screen.getByText("TLSv1.3")).toBeTruthy();
    expect(screen.getByText("h2")).toBeTruthy();
    expect(screen.getByText("Certificate chain (2)")).toBeTruthy();
    expect(screen.getByText(/expires in [45] days/)).toBeTruthy();
    expect(screen.getByText("expired", { exact: false })).toBeTruthy();
    expect(screen.getByText("api.example.test, *.example.test")).toBeTruthy();
  });
});
