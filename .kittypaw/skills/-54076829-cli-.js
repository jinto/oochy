const targetChatId = "54076829";
const message = "CLI 테스트 메시지";

try {
  await Telegram.sendMessage(targetChatId, message);
  return `메시지가 성공적으로 전송되었습니다: "${message}"`;
} catch (error) {
  console.log("메시지 전송 실패:", error);
  return `메시지 전송에 실패했습니다: ${error.message}`;
}