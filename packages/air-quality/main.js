const ctx = JSON.parse(__context__);
const config = ctx.config || {};

const telegramToken = config.telegram_token;
const chatId = config.chat_id;
const apiKey = config.api_key;
const station = config.station || "종로구";

// Fetch air quality from 에어코리아
const url = `https://apis.data.go.kr/B552584/ArpltnInforInqireSvc/getMsrstnAcctoRltmMesureDnsty?serviceKey=${encodeURIComponent(apiKey)}&returnType=json&numOfRows=1&pageNo=1&stationName=${encodeURIComponent(station)}&dataTerm=DAILY&ver=1.0`;

let data;
try {
  const raw = await Http.get(url);
  data = JSON.parse(raw);
} catch (e) {
  return `Error fetching air quality: ${e}`;
}

const items = data?.response?.body?.items;
if (!items || items.length === 0) {
  return "No air quality data available.";
}

const item = items[0];
const pm10 = item.pm10Value || "-";
const pm25 = item.pm25Value || "-";
const pm10Grade = item.pm10Grade1h || "0";
const pm25Grade = item.pm25Grade1h || "0";

const gradeMap = { "1": "좋음 😊", "2": "보통 🙂", "3": "나쁨 😷", "4": "매우나쁨 🚨" };
const pm10Text = gradeMap[pm10Grade] || "정보없음";
const pm25Text = gradeMap[pm25Grade] || "정보없음";

const needMask = parseInt(pm10Grade) >= 3 || parseInt(pm25Grade) >= 3;
const tip = needMask ? "🎭 마스크를 챙기세요!" : "✅ 야외 활동 괜찮아요.";

const message = [
  `🌫 *미세먼지 알림 — ${station}*`,
  ``,
  `미세먼지(PM10): ${pm10}㎍/㎥ — ${pm10Text}`,
  `초미세먼지(PM2.5): ${pm25}㎍/㎥ — ${pm25Text}`,
  ``,
  tip,
  ``,
  `_출처: 에어코리아 · Powered by KittyPaw_`,
].join("\n");

await Telegram.sendMessage(chatId, message, { parse_mode: "Markdown" });

return `Air quality alert sent for ${station}: PM10=${pm10}, PM2.5=${pm25}`;
