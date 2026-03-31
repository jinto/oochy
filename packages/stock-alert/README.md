# 주식 알림 (stock-alert)

평일 장중(9~16시) 30분마다 네이버 금융 API에서 지정 종목의 현재가를 조회하고, 목표 상한가 또는 하한가를 돌파했을 때 텔레그램으로 알림을 보냅니다. 중복 알림 방지를 위해 Storage에 마지막 알림 상태를 저장합니다.

## 동작 방식

1. 네이버 금융 모바일 API(`m.stock.naver.com`)에서 현재가를 조회합니다.
2. 현재가와 설정한 `target_high` / `target_low`를 비교합니다.
3. 임계값을 **처음 돌파했을 때만** 텔레그램 알림을 전송합니다 (에지 트리거).
4. Storage에 알림 상태를 저장해 같은 방향의 임계값에 대한 중복 알림을 막습니다.
5. 가격이 임계값 사이로 돌아오면 플래그를 초기화해 다음 돌파 시 재알림합니다.

## 설정

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `telegram_token` | secret | yes | — | 텔레그램 봇 토큰 |
| `chat_id` | string | yes | — | 텔레그램 채팅 또는 채널 ID |
| `stock_code` | string | no | `005930` | 종목코드 (예: 005930=삼성전자) |
| `stock_name` | string | no | `삼성전자` | 종목명 (표시용) |
| `target_high` | number | no | — | 목표 상한가 (원) |
| `target_low` | number | no | — | 목표 하한가 (원) |

## 출력 예시

```
📈 삼성전자(005930)
현재가: 82,500원
📈 상한가 목표가 80,000원 돌파!
```

```
📉 삼성전자(005930)
현재가: 68,900원
📉 하한가 목표가 70,000원 돌파!
```

## 데이터 출처

- **네이버 금융** — `https://m.stock.naver.com/api/stock/{code}/basic`
- 별도 API 키 불필요.
