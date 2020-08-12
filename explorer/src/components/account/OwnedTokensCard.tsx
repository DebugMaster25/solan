import React from "react";
import { PublicKey } from "@solana/web3.js";
import { FetchStatus } from "providers/cache";
import {
  useFetchAccountOwnedTokens,
  useAccountOwnedTokens,
} from "providers/accounts/tokens";
import { ErrorCard } from "components/common/ErrorCard";
import { LoadingCard } from "components/common/LoadingCard";
import { Address } from "components/common/Address";

export function OwnedTokensCard({ pubkey }: { pubkey: PublicKey }) {
  const address = pubkey.toBase58();
  const ownedTokens = useAccountOwnedTokens(address);
  const fetchAccountTokens = useFetchAccountOwnedTokens();
  const refresh = () => fetchAccountTokens(pubkey);

  // Fetch owned tokens
  React.useEffect(() => {
    if (!ownedTokens) refresh();
  }, [address]); // eslint-disable-line react-hooks/exhaustive-deps

  if (ownedTokens === undefined) {
    return null;
  }

  const { status } = ownedTokens;
  const tokens = ownedTokens.data?.tokens;
  const fetching = status === FetchStatus.Fetching;
  if (fetching && (tokens === undefined || tokens.length === 0)) {
    return <LoadingCard message="Loading owned tokens" />;
  } else if (tokens === undefined) {
    return <ErrorCard retry={refresh} text="Failed to fetch owned tokens" />;
  }

  if (tokens.length === 0) {
    return (
      <ErrorCard
        retry={refresh}
        retryText="Try Again"
        text={"No owned tokens found"}
      />
    );
  }

  const mappedTokens = new Map<string, number>();
  for (const { info: token } of tokens) {
    const mintAddress = token.mint.toBase58();
    const totalByMint = mappedTokens.get(mintAddress);

    let amount = token?.amount || (token?.tokenAmount?.uiAmount as number);
    if (totalByMint !== undefined) {
      amount += totalByMint;
    }

    mappedTokens.set(mintAddress, amount);
  }

  const detailsList: React.ReactNode[] = [];
  mappedTokens.forEach((totalByMint, mintAddress) => {
    detailsList.push(
      <tr key={mintAddress}>
        <td>
          <Address pubkey={new PublicKey(mintAddress)} link />
        </td>
        <td>{totalByMint}</td>
      </tr>
    );
  });

  return (
    <div className="card">
      <div className="card-header align-items-center">
        <h3 className="card-header-title">Owned Tokens</h3>
        <button
          className="btn btn-white btn-sm"
          disabled={fetching}
          onClick={refresh}
        >
          {fetching ? (
            <>
              <span className="spinner-grow spinner-grow-sm mr-2"></span>
              Loading
            </>
          ) : (
            <>
              <span className="fe fe-refresh-cw mr-2"></span>
              Refresh
            </>
          )}
        </button>
      </div>

      <div className="table-responsive mb-0">
        <table className="table table-sm table-nowrap card-table">
          <thead>
            <tr>
              <th className="text-muted">Token Address</th>
              <th className="text-muted">Balance</th>
            </tr>
          </thead>
          <tbody className="list">{detailsList}</tbody>
        </table>
      </div>
    </div>
  );
}
