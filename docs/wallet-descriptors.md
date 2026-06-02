---
title: Wallet Descriptors
description: Link qpayd to a watch-only Bitcoin wallet.
order: 10
---

# Wallet descriptors

qpayd needs a watch-only wallet descriptor for on-chain Bitcoin invoices. The
descriptor lets qpayd generate one receive address per invoice and watch those
addresses for payment. It does not let qpayd spend coins.

Address derivation state is stored in the configured database, per store. Back
up the qpayd database with the same care as other payment records so a restore
continues from the next unused address index.

## Descriptor shape

Native SegWit wallets usually use:

```text
wpkh(YOUR_XPUB/0/*)
```

Taproot wallets use:

```text
tr(YOUR_XPUB/0/*)
```

If your wallet shows a master fingerprint and account path, include them:

```text
wpkh([FINGERPRINT/84h/0h/0h]YOUR_XPUB/0/*)
```

Set the descriptor as an environment variable:

```sh
export QPAYD_MAIN_DESCRIPTOR='wpkh(xpub.../0/*)'
```

## Blockstream apps

Use a singlesig Bitcoin account.

1. Open the wallet account.
2. Open the account or wallet settings.
3. Find watch-only or extended public key export.
4. Copy the account xpub or descriptor.
5. Use `wpkh(YOUR_XPUB/0/*)` for a native SegWit account.

Do not use a Green 2FA or multisig account for the first setup. Use singlesig
so the exported xpub maps directly to invoice receive addresses.

## Sparrow

1. Open the wallet.
2. Open Settings.
3. Copy the wallet descriptor, or copy the account xpub and script type.
4. Use the descriptor directly if it ends with `/0/*`.

## Electrum

Use a standard wallet.

1. Open the wallet.
2. Open the wallet information (computer) or wallet details (Android).
3. Copy the Master Public Key.
4. Use `wpkh(YOUR_XPUB/0/*)` for a native SegWit account.

Do not use a 2FA or multisig wallet for the first setup. Use a standard wallet
so the exported xpub maps directly to invoice receive addresses.

## BlueWallet

1. Select the wallet.
2. Press/click (...) for settings.
3. Press Export/Backup if it is a watch-only wallet OR if seeded, select Show Wallet XPUB under Options.
4. Copy the account's xpub or zpub.
5. Use `wpkh(YOUR_XPUB/0/*)` for a native SegWit account.

Do not use a Multisig Valunt for the first setup. Use the default Bitcoin wallet
so the exported xpub maps directly to invoice receive addresses.

## Test the link

Before taking real payments, create a small invoice and pay it from another
wallet you control:

```sh
curl -sS http://127.0.0.1:8080/v1/stores/main/invoices \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"amount":"1.00","currency":"USD"}'
```

The response should include an `onchain_address` and `onchain_address_index`.
Send a small payment to that address, then run:

```sh
qpayd --config qpayd.toml sync-once
```

Read the invoice again and confirm it moves to `settled` after confirmation.

