import { verifyHmac } from "./hmac";

export interface Env {
  MESSAGES: KVNamespace;
  KAKAO_SECRET: string;
}

interface KakaoPayload {
  action: { id: string };
  userRequest: {
    utterance: string;
    user: { id: string };
  };
  callbackUrl: string;
}

interface StoredMessage {
  text: string;
  user_id: string;
  callback_url: string;
  action_id: string;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    // POST /webhook?token={user_token}
    if (request.method === "POST" && url.pathname === "/webhook") {
      return handleWebhook(request, env, url);
    }

    // POST /poll/{user_token}
    const pollMatch = url.pathname.match(/^\/poll\/(.+)$/);
    if (request.method === "POST" && pollMatch) {
      const userToken = pollMatch[1];
      return handlePoll(env, userToken);
    }

    return new Response("Not Found", { status: 404 });
  },
};

async function handleWebhook(
  request: Request,
  env: Env,
  url: URL
): Promise<Response> {
  const secret = env.KAKAO_SECRET;
  if (!secret) {
    return new Response("Server misconfigured: KAKAO_SECRET not set", { status: 500 });
  }

  const body = await request.text();
  const signature = request.headers.get("X-Kakao-Signature") ?? "";

  const valid = await verifyHmac(secret, body, signature);
  if (!valid) {
    return new Response("Unauthorized", { status: 401 });
  }

  let payload: KakaoPayload;
  try {
    payload = JSON.parse(body) as KakaoPayload;
  } catch {
    return new Response("Bad Request", { status: 400 });
  }

  const actionId = payload.action?.id;
  const utterance = payload.userRequest?.utterance;
  const userId = payload.userRequest?.user?.id;
  const callbackUrl = payload.callbackUrl;

  if (!actionId || !utterance || !userId || !callbackUrl) {
    return new Response("Bad Request: missing required fields", { status: 400 });
  }

  // user_token comes from the required query parameter — reject if missing
  const userToken = url.searchParams.get("token");
  if (!userToken) {
    return new Response("Bad Request: missing ?token= parameter", { status: 400 });
  }

  const msg: StoredMessage = {
    text: utterance,
    user_id: userId,
    callback_url: callbackUrl,
    action_id: actionId,
  };

  // KV key = {user_token}:{action_id} for idempotency
  // TTL: 300 seconds — generous enough to survive polling backoff (max ~60s)
  const kvKey = `${userToken}:${actionId}`;
  await env.MESSAGES.put(kvKey, JSON.stringify(msg), { expirationTtl: 300 });

  // Immediate ACK to Kakao — actual response goes via callbackUrl
  return Response.json({ version: "2.0", useCallback: true });
}

async function handlePoll(env: Env, userToken: string): Promise<Response> {
  // List keys with the user_token prefix to find pending messages
  const list = await env.MESSAGES.list({ prefix: `${userToken}:`, limit: 1 });

  if (list.keys.length === 0) {
    return new Response(null, { status: 204 });
  }

  const key = list.keys[0].name;
  const value = await env.MESSAGES.get(key);

  if (!value) {
    return new Response(null, { status: 204 });
  }

  // Atomic: delete immediately after read to prevent duplicate delivery
  await env.MESSAGES.delete(key);

  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return new Response("Internal Error: corrupted KV value", { status: 500 });
  }
  return Response.json(parsed);
}
