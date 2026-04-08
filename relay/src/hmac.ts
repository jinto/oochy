/**
 * Verify Kakao Open Builder HMAC-SHA256 signature.
 *
 * Kakao sends `X-Kakao-Signature: <base64(HMAC-SHA256(secret, body))>`.
 */
export async function verifyHmac(
  secret: string,
  body: string,
  signature: string
): Promise<boolean> {
  const encoder = new TextEncoder();
  let key: CryptoKey;
  try {
    key = await crypto.subtle.importKey(
      "raw",
      encoder.encode(secret),
      { name: "HMAC", hash: "SHA-256" },
      false,
      ["sign"]
    );
  } catch {
    return false;
  }

  const expectedBytes = await crypto.subtle.sign(
    "HMAC",
    key,
    encoder.encode(body)
  );
  const expected = btoa(
    String.fromCharCode(...new Uint8Array(expectedBytes))
  );

  // Constant-time comparison to prevent timing attacks
  if (expected.length !== signature.length) return false;
  let mismatch = 0;
  for (let i = 0; i < expected.length; i++) {
    mismatch |= expected.charCodeAt(i) ^ signature.charCodeAt(i);
  }
  return mismatch === 0;
}
