// @flow

import {Account, PublicKey, Loader, SystemProgram} from '.';
import {sendAndConfirmTransaction} from './util/send-and-confirm-transaction';
import type {Connection} from '.';

/**
 * Factory class for transactions to interact with a program loader
 */
export class NativeLoader {
  /**
   * Public key that identifies the NativeLoader
   */
  static get programId(): PublicKey {
    return new PublicKey('0x0202020202020202020202020202020202020202020202020202020202020202');
  }

  /**
   * Loads a native program
   *
   * @param connection The connection to use
   * @param owner User account to load the program with
   * @param programName Name of the native program
   */
  static async load(
    connection: Connection,
    owner: Account,
    programName: string,
  ): Promise<PublicKey> {
    const bytes = [...Buffer.from(programName)];

    const programAccount = new Account();

    // Allocate memory for the program account
    const transaction = SystemProgram.createAccount(
      owner.publicKey,
      programAccount.publicKey,
      1,
      bytes.length + 1,
      NativeLoader.programId,
    );
    await sendAndConfirmTransaction(connection, owner, transaction);

    const loader = new Loader(connection, NativeLoader.programId);
    await loader.load(programAccount, 0, bytes);
    await loader.finalize(programAccount);

    return programAccount.publicKey;
  }
}
