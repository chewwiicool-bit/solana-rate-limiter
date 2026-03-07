# Solana Rate Limiter — On-Chain Backend Pattern

> **Superteam Earn: "Rebuild production backend systems as on-chain Rust programs"**

A production-quality **rate limiter** implemented as a native Solana program — demonstrating how a fundamental Web2 backend primitive maps onto Solana's account model.

---

## 🌐 How This Works in Web2

Rate limiting is ubiquitous in Web2. Every API — from Stripe to GitHub to your own microservices — uses it to prevent abuse. The classic implementation:

```
# Redis-based fixed-window rate limiter (Python pseudocode)
def check_rate_limit(client_id: str, service: str) -> bool:
    key = f"rl:{service}:{client_id}"
    window_key = f"{key}:{current_window()}"
    
    count = redis.incr(window_key)
    if count == 1:
        redis.expire(window_key, WINDOW_SECONDS)
    
    return count <= MAX_CALLS
```

**Web2 components:**
- **Config store** → environment variables / service config DB
- **Rate limit logic** → Redis INCR + EXPIRE, or Nginx `limit_req`
- **Per-client state** → Redis key `rl:{service}:{client_ip}`
- **Authority** → API gateway / middleware (implicit trust)

---

## ⛓️ How This Works on Solana

On Solana, **accounts are the database**. Instead of Redis keys, we use PDAs (Program Derived Addresses) — deterministic, on-chain state owned by our program.

### Account Model

```
Program: <PROGRAM_ID>
│
├── Config PDA          seeds: ["config"]
│   ├── authority: Pubkey      (who controls the system)
│   ├── default_max_calls: u64
│   └── default_window_secs: u64
│
├── Service PDA         seeds: ["service", service_id[32]]
│   ├── service_id: [u8; 32]   (fixed-size ID, UTF-8 padded)
│   ├── max_calls: u64
│   ├── window_secs: u64
│   └── active: bool
│
└── ClientRecord PDA    seeds: ["client", service_id[32], client_pubkey]
    ├── call_count: u64
    ├── window_start: i64      (Unix timestamp)
    └── last_call: i64
```

### Instructions

| # | Instruction | Signers | Description |
|---|-------------|---------|-------------|
| 0 | `Initialize` | authority | Create Config PDA, set global defaults |
| 1 | `RegisterService` | authority | Create Service PDA with custom limits |
| 2 | `CheckRateLimit` | caller | Enforce limit; creates ClientRecord on first call |
| 3 | `ResetClient` | authority | Admin reset of a specific client |
| 4 | `UpdateService` | authority | Change limits or pause/resume a service |

### Rate Limiting Logic (fixed window)

```rust
// Reset window if expired
if now - record.window_start >= svc.window_secs as i64 {
    record.call_count = 0;
    record.window_start = now;
}

if record.call_count >= svc.max_calls {
    return Err(RateLimiterError::RateLimitExceeded.into());
}

record.call_count += 1;
```

**Web2 → Solana mapping:**
| Web2 | Solana |
|------|--------|
| Redis `INCR` | Increment `call_count` in ClientRecord PDA |
| Redis `EXPIRE` | Check `clock.unix_timestamp - window_start` |
| Config file | Config PDA on-chain |
| Nginx service config | Service PDA per service |
| Client IP | Caller's Pubkey |
| API Gateway auth | `is_signer` check on accounts |

---

## ⚖️ Tradeoffs & Constraints

### ✅ Advantages over Web2

| Aspect | Web2 | Solana |
|--------|------|--------|
| **Verifiability** | Trust the API gateway | On-chain, auditable by anyone |
| **Manipulation** | Operator can bypass | Program logic is immutable |
| **Cross-service** | Separate rate limit DBs | Single program, shared state |
| **Payment integration** | Separate billing system | Native token gating possible |

### ⚠️ Constraints & Tradeoffs

**1. Transaction cost per check**
Every `CheckRateLimit` call costs ~0.000005 SOL (~$0.001). In Web2, Redis calls are free. This makes Solana rate limiting economically viable only for high-value API calls (DeFi transactions, NFT mints) — not for commodity APIs.

**2. Latency**
Solana's block time is ~400ms. Web2 Redis round-trips are <1ms. For real-time rate limiting of web requests, this is too slow. But for on-chain actions (contract calls, token transfers), the overhead is negligible since the transaction itself already takes 400ms.

**3. Clock precision**
`Clock::unix_timestamp` updates every slot (~400ms). Sub-second rate limiting windows are not possible.

**4. No sliding window (by design)**
This implements a fixed window algorithm for simplicity and determinism. Sliding window would require storing per-call timestamps — expensive in account space.

**5. ClientRecord rent**
Each new caller-service pair creates a 97-byte PDA costing ~0.0008 SOL in rent. The caller pays this. For the first call, the transaction is slightly more expensive.

### 🎯 Ideal Use Cases

- **Rate-limiting DeFi actions** (max 5 swaps/minute per wallet)
- **NFT mint gates** (max 1 mint per address per hour)
- **DAO governance** (max 1 proposal per week per member)
- **On-chain API keys** (pay-per-call with rate limits)

---

## 🚀 Deployment

### Devnet Program ID
```
<PROGRAM_ID_AFTER_DEPLOY>
```

### Deployment Transactions
- Initialize: `<TX_HASH>`
- RegisterService (api-v1, 3 calls/60s): `<TX_HASH>`
- Test CheckRateLimit (x3 pass + x1 reject): `<TX_HASH>`

*Full deployment logs available in GitHub Actions run.*

---

## 🛠️ Build & Run

```bash
# Build (requires Solana toolchain)
cargo build-sbf

# Deploy to devnet
solana program deploy target/deploy/solana_rate_limiter.so

# Run smoke tests
cd client && npm install
PROGRAM_ID=<your_program_id> node test.js
```

### GitHub Actions (automated)
Push to `main` → automatically builds and deploys to devnet. See `.github/workflows/build-deploy.yml`.

---

## 📁 Structure

```
solana-rate-limiter/
├── src/
│   └── lib.rs              # Full program (instructions, state, handlers)
├── client/
│   ├── test.js             # Smoke test (initialize → register → check x4)
│   └── package.json
├── .github/workflows/
│   └── build-deploy.yml    # Auto build + deploy to devnet
├── Cargo.toml
└── README.md               # This file
```

---

*Built for Superteam Earn — "Rebuild production backend systems as on-chain Rust programs"*
