// stock-alert/main.js
// Fetches the current stock price from Naver Finance API and sends a
// Telegram alert when the price crosses a configured high or low threshold.
// Uses Storage to avoid duplicate alerts for the same threshold crossing.

const ctx = JSON.parse(__context__);
const config = ctx.config || {};

const telegramToken = config.telegram_token;
const chatId = config.chat_id;
const stockCode = config.stock_code || "005930";
const stockName = config.stock_name || "삼성전자";
const targetHigh = config.target_high != null ? Number(config.target_high) : null;
const targetLow = config.target_low != null ? Number(config.target_low) : null;

// --- Fetch current price from Naver Finance mobile API ---
const apiUrl = `https://m.stock.naver.com/api/stock/${stockCode}/basic`;

let stockData;
try {
  const raw = await Http.get(apiUrl);
  stockData = JSON.parse(raw);
} catch (e) {
  return `Error fetching stock data for ${stockCode}: ${e}`;
}

const currentPrice = Number(stockData.currentPrice || stockData.closePrice || 0);
if (!currentPrice) {
  return `Could not parse current price from API response.`;
}

// --- Load last alert state from Storage ---
const storageKey = `stock-alert:${stockCode}:last_alert`;
let lastAlertRaw;
try {
  lastAlertRaw = await Storage.get(storageKey);
} catch (_) {
  lastAlertRaw = null;
}

const lastAlert = lastAlertRaw ? JSON.parse(lastAlertRaw) : { high: false, low: false };

// Determine if a threshold has been crossed (edge-trigger: only alert once per crossing)
let triggered = false;
let direction = "";
let targetPrice = 0;
let emoji = "";

if (targetHigh !== null && currentPrice >= targetHigh && !lastAlert.high) {
  triggered = true;
  direction = "상한가";
  targetPrice = targetHigh;
  emoji = "📈";
  lastAlert.high = true;
  lastAlert.low = false; // reset low so it can re-trigger after recovery
} else if (targetLow !== null && currentPrice <= targetLow && !lastAlert.low) {
  triggered = true;
  direction = "하한가";
  targetPrice = targetLow;
  emoji = "📉";
  lastAlert.low = true;
  lastAlert.high = false; // reset high so it can re-trigger after recovery
} else {
  // Price is back between thresholds — reset flags so next crossing re-alerts
  if (targetHigh !== null && currentPrice < targetHigh) lastAlert.high = false;
  if (targetLow !== null && currentPrice > targetLow) lastAlert.low = false;
}

// Persist updated alert state
try {
  await Storage.set(storageKey, JSON.stringify(lastAlert));
} catch (_) {
  // Non-fatal: proceed even if storage write fails
}

if (!triggered) {
  return `${stockName}(${stockCode}) 현재가 ${currentPrice.toLocaleString()}원 — 임계값 미도달, 알림 없음.`;
}

// --- Format and send Telegram alert ---
const priceFormatted = currentPrice.toLocaleString("ko-KR");
const targetFormatted = targetPrice.toLocaleString("ko-KR");

const message =
  `${emoji} *${stockName}\\(${stockCode}\\)*\n` +
  `현재가: *${priceFormatted}원*\n` +
  `${emoji} ${direction} 목표가 ${targetFormatted}원 돌파\\!`;

await Telegram.sendMessage(telegramToken, chatId, message, { parse_mode: "MarkdownV2" });

return `Alert sent: ${stockName}(${stockCode}) ${direction} @ ${priceFormatted}원 (target: ${targetFormatted}원).`;
