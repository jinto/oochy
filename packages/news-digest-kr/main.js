// news-digest-kr/main.js
// Fetches top Korean news from Google News RSS, summarizes each headline
// with LLM in one sentence, and sends the digest to Telegram.

const ctx = JSON.parse(__context__);
const config = ctx.config || {};

const telegramToken = config.telegram_token;
const chatId = config.chat_id;
const category = config.category || "경제";
const count = config.count || 5;

// RSS feed URLs by category
const RSS_FEEDS = {
  "정치": "https://news.google.com/rss/topics/CAAqIQgKIhtDQkFTRGdvSUwyMHZNRFZxWldZU0FtdHZLQUFQAQ?hl=ko&gl=KR&ceid=KR:ko",
  "경제": "https://news.google.com/rss/topics/CAAqIQgKIhtDQkFTRGdvSUwyMHZNRGx6TVdZU0FtdHZLQUFQAQ?hl=ko&gl=KR&ceid=KR:ko",
  "사회": "https://news.google.com/rss/topics/CAAqIQgKIhtDQkFTRGdvSUwyMHZNREpxWldZU0FtdHZLQUFQAQ?hl=ko&gl=KR&ceid=KR:ko",
  "IT과학": "https://news.google.com/rss/topics/CAAqJAgKIh5DQkFTRUFvSUwyMHZNRGRqTVhZU0FtdHZLQUFQAQ?hl=ko&gl=KR&ceid=KR:ko",
  "세계": "https://news.google.com/rss/topics/CAAqIQgKIhtDQkFTRGdvSUwyMHZNRFp5Y1dZU0FtdHZLQUFQAQ?hl=ko&gl=KR&ceid=KR:ko",
};

const feedUrl = RSS_FEEDS[category] ||
  "https://news.google.com/rss/topics/CAAqIQgKIhtDQkFTRGdvSUwyMHZNRGx6TVdZU0FtdHZLQUFQAQ?hl=ko&gl=KR&ceid=KR:ko";

// --- Fetch RSS feed ---
let xmlText;
try {
  xmlText = await Http.get(feedUrl);
} catch (e) {
  return `Error fetching news feed: ${e}`;
}

// Parse <item><title> entries from RSS XML
const itemRegex = /<item>([\s\S]*?)<\/item>/g;
const titleRegex = /<title><!\[CDATA\[(.*?)\]\]><\/title>|<title>(.*?)<\/title>/;

const titles = [];
let match;
while ((match = itemRegex.exec(xmlText)) !== null && titles.length < count) {
  const itemXml = match[1];
  const titleMatch = titleRegex.exec(itemXml);
  if (titleMatch) {
    const title = (titleMatch[1] || titleMatch[2] || "").trim();
    if (title) titles.push(title);
  }
}

if (titles.length === 0) {
  return "No news items found in feed.";
}

// --- Ask LLM to summarize each headline in one sentence ---
const prompt =
  `다음 뉴스 제목들을 각각 한 줄로 요약해주세요.\n` +
  `번호를 붙여서 응답하되, 각 줄은 "1. 요약내용" 형식으로 작성해주세요.\n\n` +
  titles.map((t, i) => `${i + 1}. ${t}`).join("\n");

let summaryLines = titles; // fallback to raw titles
try {
  const llmRaw = await Llm.generate(prompt);
  const llmData = JSON.parse(llmRaw);
  const text = llmData.text || "";
  const parsed = text
    .split("\n")
    .map(line => line.replace(/^\d+\.\s*/, "").trim())
    .filter(line => line.length > 0);
  if (parsed.length >= titles.length) {
    summaryLines = parsed.slice(0, titles.length);
  }
} catch (e) {
  // use raw titles as fallback
}

// --- Format and send Telegram message ---
const today = new Date().toLocaleDateString("ko-KR", {
  year: "numeric",
  month: "long",
  day: "numeric",
});

const numbered = summaryLines
  .map((line, i) => `${i + 1}\\. ${line}`)
  .join("\n");

const message =
  `📰 *오늘의 뉴스 ${count}선 \\(${category}\\)*\n` +
  `_${today}_\n\n` +
  `${numbered}\n\n` +
  `_출처: Google 뉴스 · Powered by KittyPaw_`;

await Telegram.sendMessage(telegramToken, chatId, message, { parse_mode: "MarkdownV2" });

return `News digest sent: ${summaryLines.length} items (${category}).`;
