import type { PublicKey, Transaction } from "@solana/web3.js";

export interface Wallet {
  publicKey: PublicKey;
  signTransaction(tx: Transaction): Promise<Transaction>;
}
interface InjectedWallet {
  publicKey?: PublicKey | null;
  connect(): Promise<unknown>;
  signTransaction(tx: Transaction): Promise<Transaction>;
}
declare global {
  interface Window {
    phantom?: { solana?: InjectedWallet };
    solflare?: InjectedWallet;
  }
}

export async function getWallet(): Promise<Wallet> {
  const wallet = window.phantom?.solana ?? window.solflare;
  if (!wallet) throw new Error("Install Phantom or Solflare");
  await wallet.connect();
  if (!wallet.publicKey) throw new Error("Wallet did not provide a public key");
  return { publicKey: wallet.publicKey, signTransaction: tx => wallet.signTransaction(tx) };
}
