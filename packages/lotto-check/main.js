// lotto-check/main.js
// Fetches the latest Lotto 6/45 draw results from dhlottery.co.kr,
// compares them with the user's numbers, and sends the result to Telegram.

const ctx = JSON.parse(__context__);
const config = ctx.config || {};

const telegramToken = config.telegram_token;
const chatId = config.chat_id;
const myNumbersRaw = config.my_numbers || "";

// Parse user's numbers
const myNumbers = myNumbersRaw
  .split(",")
  .map((n) => parseInt(n.trim(), 10))
  .filter((n) => !isNaN(n));

if (myNumbers.length !== 6) {
  return `설정 오류: 숫자 6개를 쉼표로 구분해 입력해주세요. 현재: "${myNumbersRaw}"`;
}

// Calculate the latest draw number based on the first draw date (2002-12-07)
const firstDrawDate = new Date("2002-12-07").getTime();
const now = Date.now();
const weekMs = 7 * 24 * 60 * 60 * 1000;
const drawRound = Math.floor((now - firstDrawDate) / weekMs) + 1;

// --- Fetch draw results ---
const url = `https://www.dhlottery.co.kr/common.do?method=getLottoNumber&drwNo=${drawRound}`;

let data;
try {
  const raw = await Http.get(url);
  data = JSON.parse(raw);
} catch (e) {
  return `Error fetching lotto data: ${e}`;
}

if (!data || data.returnValue !== "success") {
  return `Lotto API error or draw not yet available for round ${drawRound}.`;
}

const winNumbers = [
  data.drwtNo1,
  data.drwtNo2,
  data.drwtNo3,
  data.drwtNo4,
  data.drwtNo5,
  data.drwtNo6,
];
const bonusNumber = data.bnusNo;
const round = data.drwNo;

// Count matches
const matchCount = myNumbers.filter((n) => winNumbers.includes(n)).length;
const hasBonus = myNumbers.includes(bonusNumber);

// Determine prize rank
let prize;
if (matchCount === 6) {
  prize = "🏆 1등!";
} else if (matchCount === 5 && hasBonus) {
  prize = "🥈 2등!";
} else if (matchCount === 5) {
  prize = "🥉 3등";
} else if (matchCount === 4) {
  prize = "4등";
} else if (matchCount === 3) {
  prize = "5등";
} else {
  prize = "낙첨";
}

const winStr = winNumbers.join(", ");
const myStr = myNumbers.join(", ");

const message = [
  `🎰 *로또 ${round}회 당첨 번호*`,
  `${winStr} + 보너스 ${bonusNumber}`,
  ``,
  `내 번호: ${myStr}`,
  `일치: ${matchCount}개 — ${prize}`,
].join("\n");

await Telegram.sendMessage(telegramToken, chatId, message, { parse_mode: "Markdown" });

return `Lotto check done. Round ${round}: matched ${matchCount} numbers. Prize: ${prize}`;
