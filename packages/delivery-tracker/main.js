// delivery-tracker/main.js
// Checks parcel delivery status via 스마트택배 API every 2 hours.
// Sends a Telegram notification only when the status changes.

const ctx = JSON.parse(__context__);
const config = ctx.config || {};

const telegramToken = config.telegram_token;
const chatId = config.chat_id;
const carrier = config.carrier;
const trackingNumber = config.tracking_number;
const apiKey = config.api_key;

// Map carrier names to 스마트택배 carrier codes
const CARRIER_CODES = {
  "CJ대한통운": "04",
  "한진택배": "05",
  "롯데택배": "08",
  "우체국": "01",
};

const carrierCode = CARRIER_CODES[carrier];
if (!carrierCode) {
  return `Unknown carrier: ${carrier}`;
}

// --- Fetch tracking info from 스마트택배 API ---
const url =
  `https://info.sweettracker.co.kr/api/v1/trackingInfo` +
  `?t_key=${encodeURIComponent(apiKey)}` +
  `&t_code=${carrierCode}` +
  `&t_invoice=${encodeURIComponent(trackingNumber)}`;

let data;
try {
  const raw = await Http.get(url);
  data = JSON.parse(raw);
} catch (e) {
  return `Error fetching tracking data: ${e}`;
}

if (!data || data.status === false) {
  const msg = data && data.msg ? data.msg : "Unknown error";
  return `Tracking API error: ${msg}`;
}

const trackingDetails = data.trackingDetails;
if (!trackingDetails || trackingDetails.length === 0) {
  return `No tracking details available for ${trackingNumber}.`;
}

// Latest status is the last entry
const latest = trackingDetails[trackingDetails.length - 1];
const latestStatus = latest.kind || latest.level || "상태 불명";
const latestTime = latest.timeString || latest.time || "";

// --- Check for status change via Storage ---
const storageKey = `delivery-tracker:${trackingNumber}:last_status`;
let lastStatus = "";
try {
  const stored = await Storage.get(storageKey);
  if (stored) {
    lastStatus = stored;
  }
} catch (_) {
  // No previous status stored — first run
}

if (latestStatus === lastStatus) {
  return `No status change for ${trackingNumber}. Current: ${latestStatus}`;
}

// Status changed — update storage and send notification
await Storage.set(storageKey, latestStatus);

const message = [
  `📦 *배송 조회* — ${carrier}`,
  `${trackingNumber}`,
  ``,
  `현재: ${latestStatus}`,
  `${latestTime}`,
].join("\n");

await Telegram.sendMessage(telegramToken, chatId, message, { parse_mode: "Markdown" });

return `Notification sent. Status changed from "${lastStatus || "(none)"}" to "${latestStatus}".`;
