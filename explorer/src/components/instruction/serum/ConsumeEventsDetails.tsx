import React from "react";
import { SignatureResult, TransactionInstruction } from "@solana/web3.js";
import { InstructionCard } from "../InstructionCard";
import { Address } from "components/common/Address";
import { ConsumeEvents } from "./types";

export function ConsumeEventsDetailsCard(props: {
  ix: TransactionInstruction;
  index: number;
  result: SignatureResult;
  info: ConsumeEvents;
}) {
  const { ix, index, result, info } = props;

  return (
    <InstructionCard
      ix={ix}
      index={index}
      result={result}
      title="Serum: Consume Events"
    >
      <tr>
        <td>Program</td>
        <td className="text-lg-right">
          <Address pubkey={info.programId} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Market</td>
        <td className="text-lg-right">
          <Address pubkey={info.market} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Event Queue</td>
        <td className="text-lg-right">
          <Address pubkey={info.eventQueue} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Open Orders Accounts</td>
        <td className="text-lg-right">
          {info.openOrdersAccounts.map((account, index) => {
            return <Address pubkey={account} key={index} alignRight link />;
          })}
        </td>
      </tr>

      <tr>
        <td>Limit</td>
        <td className="text-lg-right">{info.limit}</td>
      </tr>
    </InstructionCard>
  );
}
