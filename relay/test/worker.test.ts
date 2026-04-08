import {
  env,
  createExecutionContext,
  waitOnExecutionContext,
  SELF,
} from "cloudflare:test";
import { describe, it, expect, beforeAll } from "vitest";

// Test payload matching Kakao Open Builder format
const testPayload = {
  action: { id: "test_action_id" },
  userRequest: {
    utterance: "안녕하세요",
    user: { id: "test_user_key" },
  },
  callbackUrl: "https://callback.kakao.com/test",
};

const TEST_SECRET = "test_hmac_secret";

async function computeHmac(secret: string, body: string): Promise<string> {
  const encoder = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const signature = await crypto.subtle.sign("HMAC", key, encoder.encode(body));
  return btoa(String.fromCharCode(...new Uint8Array(signature)));
}

describe("POST /webhook", () => {
  it("rejects invalid HMAC with 401", async () => {
    const body = JSON.stringify(testPayload);
    const res = await SELF.fetch("http://example.com/webhook", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Kakao-Signature": "invalid_signature",
      },
      body,
    });
    expect(res.status).toBe(401);
  });

  it("rejects missing token with 400", async () => {
    const body = JSON.stringify(testPayload);
    const sig = await computeHmac(TEST_SECRET, body);
    const res = await SELF.fetch("http://example.com/webhook", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Kakao-Signature": sig,
      },
      body,
    });
    expect(res.status).toBe(400);
  });

  it("accepts valid HMAC and returns useCallback response", async () => {
    const body = JSON.stringify(testPayload);
    const sig = await computeHmac(TEST_SECRET, body);
    const res = await SELF.fetch("http://example.com/webhook?token=test_user_key", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Kakao-Signature": sig,
      },
      body,
    });
    expect(res.status).toBe(200);
    const json = await res.json<{ version: string; useCallback: boolean }>();
    expect(json.version).toBe("2.0");
    expect(json.useCallback).toBe(true);
  });

  it("stores message in KV with correct key", async () => {
    const body = JSON.stringify(testPayload);
    const sig = await computeHmac(TEST_SECRET, body);
    await SELF.fetch("http://example.com/webhook?token=test_user_key", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Kakao-Signature": sig,
      },
      body,
    });
    // KV key = {user_token}:{action_id}
    const stored = await env.MESSAGES.get("test_user_key:test_action_id");
    expect(stored).not.toBeNull();
    const msg = JSON.parse(stored!);
    expect(msg.text).toBe("안녕하세요");
    expect(msg.user_id).toBe("test_user_key");
    expect(msg.callback_url).toBe("https://callback.kakao.com/test");
  });
});

describe("POST /poll/:user_token", () => {
  it("returns 204 when no messages", async () => {
    const res = await SELF.fetch("http://example.com/poll/empty_user", {
      method: "POST",
    });
    expect(res.status).toBe(204);
  });

  it("returns stored message and deletes it", async () => {
    // Pre-seed KV
    await env.MESSAGES.put(
      "poll_user:act99",
      JSON.stringify({
        text: "테스트 메시지",
        user_id: "poll_user",
        callback_url: "https://cb.kakao.com/99",
        action_id: "act99",
      }),
      { expirationTtl: 60 }
    );

    const res = await SELF.fetch("http://example.com/poll/poll_user", {
      method: "POST",
    });
    expect(res.status).toBe(200);
    const msg = await res.json<{
      text: string;
      user_id: string;
      callback_url: string;
      action_id: string;
    }>();
    expect(msg.text).toBe("테스트 메시지");
    expect(msg.user_id).toBe("poll_user");

    // Message should be deleted after claim
    const after = await env.MESSAGES.get("poll_user:act99");
    expect(after).toBeNull();
  });
});
