# Veloren Devnet SPL Marketplace

This service prepares and verifies a devnet-only SPL NFT purchase transaction.
It does not mint NFTs and it does not expose the seller's private key over HTTP.

The configured seller keypair signs the NFT transfer server-side. The buyer
wallet signs the returned partially signed transaction in Phantom. The
transaction atomically pays the seller in SOL and transfers one land NFT to
the buyer.

## Configuration

Copy `.env.example` into your deployment environment. Set:

- `VELOREN_SOLANA_RPC_URL`
- `VELOREN_MARKETPLACE_SELLER`
- `VELOREN_MARKETPLACE_SELLER_KEYPAIR`
- `VELOREN_MARKETPLACE_LAND_MINT`
- `VELOREN_MARKETPLACE_PRICE_LAMPORTS`

The keypair value must be a JSON byte array. Keep it in a deployment secret
store and never commit it.

## Endpoints

- `GET /marketplace/listings`
- `POST /marketplace/{parcel_id}/purchase`
  - Body: `{ "buyer": "<public key>" }`
  - Returns a base64 serialized, seller-partially-signed transaction.
- `POST /marketplace/{parcel_id}/confirm`
  - Body: `{ "buyer": "<public key>", "signature": "<transaction signature>" }`
  - Confirms the transaction and verifies the buyer owns the configured mint.

The game server should only grant property ownership after `/confirm` returns
`verified: true`.

The desktop client purchase button opens the local Phantom approval bridge at
`127.0.0.1:38292` and uses `VELOREN_MARKETPLACE_API_URL` to locate this service.
The bridge returns only the public wallet and confirmed transaction signature;
it never receives a private key.

This first implementation is intentionally a custodial devnet service. Before
production deployment, move the seller authorization into a deployed escrow
program and add authenticated callbacks between this service and Veloren.

## Devnet setup

Create a dedicated seller keypair and fund it with devnet SOL:

```bash
solana-keygen new --outfile marketplace-seller.json
solana config set --url devnet
solana airdrop 2 "$(solana-keygen pubkey marketplace-seller.json)"
```

Mint a one-supply, zero-decimal SPL token for each development parcel, then
send the land mint to the seller's associated token account. The seller
wallet must hold exactly one token for the configured mint before a purchase
request is accepted.

The service does not mint, transfer, or expose key material through HTTP.
Generate the environment value from the deployment secret store rather than
committing it:

```bash
export VELOREN_MARKETPLACE_SELLER_KEYPAIR="$(python3 -c 'import json; print(json.dumps(json.load(open("marketplace-seller.json"))))')"
cargo run -p veloren-marketplace-service
```
