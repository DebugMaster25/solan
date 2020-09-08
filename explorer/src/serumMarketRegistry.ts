import { Cluster } from "providers/cluster";

const MARKET_REGISTRY: { [key: string]: string } = {
  "7kgkDyW7dmyMeP8KFXzbcUZz1R2WHsovDZ7n3ihZuNDS": "Serum: MSRM / USDC",
  "6WpkeqE5oU1MUNPWvMDHhru1G5gjxMgAtib5wXuBSvgm": "Serum: MSRM / USDC - Request Queue",
  "DwpUjRHQuotE1LG2R68wZM3nwkv2fChHibcm7NzL8WGq": "Serum: MSRM / USDC - Event Queue",
  "7zyPwxjHMJsTdPe7Rd992oe1cVhrZcbkcH9qURzKV8wV": "Serum: MSRM / USDC - Order Book (Bids)",
  "4nHe9oNh7JJoJZ1HrktVghB19Cis4N848so7UCWXhF2t": "Serum: MSRM / USDC - Order Book (Asks)",
  "8gbnu8XUNmigCSKP43UXbtYYTUHJPRbctyB7Kj1QbTyZ": "Serum: MSRM / USDC - Base Asset Vault",
  "8MboeurJ28fQj3n18jBrM7oQSu3coyFbQUpxWnabF3gc": "Serum: MSRM / USDC - Quote Currency Vault",
  "GFyDCG3EBVrAWiHKLf7zF2DLqMp89dLHUtsYKwFUe4AC": "Serum: MSRM / USDC - Vault Owner",

  "H4snTKK9adiU15gP22ErfZYtro3aqR9BTMXiH3AwiUTQ": "Serum: MSRM / USDT",
  "3mrit8EKnsy9M8L7EQ24GjNeuwXssVGqDRLZXDarb9Wk": "Serum: MSRM / USDT - Request Queue",
  "98qQ1Dintci8xSUAHEJqQukSKnE9g1LQY2jxNwwgcQQu": "Serum: MSRM / USDT - Event Queue",
  "83Snk2SJTX8KKMPmi5UX9JYKxy2QWrn2jFC9zH6NmP7L": "Serum: MSRM / USDT - Order Book (Bids)",
  "CKjPS8ntVbN7YjGk4Goq24ScaA1qjNFFqQpVuzNkFRo4": "Serum: MSRM / USDT - Order Book (Asks)",
  "mKeCpjYQWzptDQn5J5XfSzyoNmsnH9W3RryzUWFe3G7": "Serum: MSRM / USDT - Base Asset Vault",
  "9o8XKrbbbA8eXMe72Tsopkk8y7aFF2HQPDMoxdyX4S9b": "Serum: MSRM / USDT - Quote Currency Vault",
  "12rqwuEgBYiGhBrDJStCiqEtzQpTTiZbh7teNVLuYcFA": "Serum: MSRM / USDT - Vault Owner",

  "CAgAeMD7quTdnr6RPa7JySQpjf3irAmefYNdTb6anemq": "Serum: BTC / USDC",
  "5PMuDUdk7VFLSYXDo6wHsEfyfWAf1rG4rkdmrmnK4ZME": "Serum: BTC / USDC - Request Queue",
  "DrGCgNJAwpihVrRCzd69Ys6k5ggu1qC2FQFjCESnj3Do": "Serum: BTC / USDC - Event Queue",
  "6oVGgm4D2fgvv3jcTy3DzUHCbu14J6pe7RYqxHA5FGB1": "Serum: BTC / USDC - Order Book (Bids)",
  "AXjn1qHYrAad5nVznXm7Txyp8WmdzJEEi5NM2axSLtnL": "Serum: BTC / USDC - Order Book (Asks)",
  "CAiSJtSZfA5usmP9ikPz69nzJLMejbjo7CrGQLwSXR8h": "Serum: BTC / USDC - Base Asset Vault",
  "HcEnLVJxG126a68FX6pzw252pL9jUrxkucsbth8hzh9U": "Serum: BTC / USDC - Quote Currency Vault",
  "3bYr6BzwMqrrH1N3BGjYeEPRrRxde25zj3Garud4qom3": "Serum: BTC / USDC - Vault Owner",

  "8AcVjMG2LTbpkjNoyq8RwysokqZunkjy3d5JDzxC6BJa": "Serum: BTC / USDT",
  "DoYVFapBvZksM6DqqsWgLrqYQL5JknhnjFcEPb2jCr6D": "Serum: BTC / USDT - Request Queue",
  "6iveBLh1CWrE1T4p7AE1tX3DPSpnbPLxCPZ2RxTfeTvT": "Serum: BTC / USDT - Event Queue",
  "5N2uRKMNDv22rgV8Cm8VtyTByvKGfdFBTtUToqga3bxR": "Serum: BTC / USDT - Order Book (Bids)",
  "7g8U7LqXbMcRyLnU3KhDNJKrq4VTDbvcVTL3Qt1onKJD": "Serum: BTC / USDT - Order Book (Asks)",
  "5piQPUuxpGR1HVDA7vMJneH3bcogmXJo7crgNmwaP49D": "Serum: BTC / USDT - Base Asset Vault",
  "5Gb6ngdANPa3upkaGbcR9ib55PcsktDhiQcsVr3XCZhj": "Serum: BTC / USDT - Quote Currency Vault",
  "4rJea7ape61LD6CredX9Xd5mWpKQiJrcWwencsUe1s5r": "Serum: BTC / USDT - Vault Owner",

  "ASKiV944nKg1W9vsf7hf3fTsjawK6DwLwrnB2LH9n61c": "Serum: ETH / USDC",
  "BWqhtYi9sQgN8wfsJNBkKA5fMmbmctFwjUL1xZSHEEdm": "Serum: ETH / USDC - Request Queue",
  "PzjG3J5Lzm2P9BaeHHogxacp1BDMtawvrz88fWuadT4": "Serum: ETH / USDC - Event Queue",
  "9HqtE85iR2DFMDduCAZe9DG9ygckE7WgmVmbdvKVj3x1": "Serum: ETH / USDC - Order Book (Bids)",
  "tSZM7972KpDL1WV2r9fTwGhaoPWhJ8nMnEzuvhW9b6Y": "Serum: ETH / USDC - Order Book (Asks)",
  "HSzt9nNG6MuD1iojc76Ke3eeZ9tN4nnd2NduR3ZccE48": "Serum: ETH / USDC - Base Asset Vault",
  "Hmxx3y5EiaaYrC65uoxmsP97TTDs7ffHAi44oiKxTypU": "Serum: ETH / USDC - Quote Currency Vault",
  "4PfXQMYgQPZe7Z6SuAFKmRiM2jNMi9XbN2vbMS2BhwSm": "Serum: ETH / USDC - Vault Owner",

  "HfCZdJ1wfsWKfYP2qyWdXTT5PWAGWFctzFjLH48U1Hsd": "Serum: ETH / USDT",
  "78dUr2kcuvDnsxTb8caFaGeD8GBmreReSLevDEJjVJmf": "Serum: ETH / USDT - Request Queue",
  "2GQd5e5z5BvsPyqSVVUs5Qgk8uSPf43MWnKAMfEQRPH7": "Serum: ETH / USDT - Event Queue",
  "888UP9HWwY1vaCMpNpP4nzQXgn7hpqomaFwQcV95FkYB": "Serum: ETH / USDT - Order Book (Bids)",
  "RCDCybQ7ivCzZPLWQWXNmQo1b6kyAbrkYZStnpJC3Dd": "Serum: ETH / USDT - Order Book (Asks)",
  "E67Asz53dn73aw7Eju1UdL72qVGTK4Svv3Bk7BYdmwTa": "Serum: ETH / USDT - Base Asset Vault",
  "J9EEc8dgJyygNWwN8xzsMh342hS58JRyuX4mBqKNUyQn": "Serum: ETH / USDT - Quote Currency Vault",
  "9UY946SraowE1R97at4qc8NXfJdr75mBLTbjesN4JKb8": "Serum: ETH / USDT - Vault Owner",

  "68J6nkWToik6oM9rTatKSR5ibVSykAtzftBUEAvpRsys": "Serum: SRM / USDC",
  "GPwEA9RMXAk2ovJZke6xczWt9HM9NzAAC5GD8JeTNRfL": "Serum: SRM / USDC - Request Queue",
  "2N1a9yuTRePYzjszUHr2qyjhrKdH3UqUosEbW9sRm5Sq": "Serum: SRM / USDC - Event Queue",
  "GKiV6ogZunNyk5nhcRZLTmDbXztDynyB674apQgGo5ve": "Serum: SRM / USDC - Order Book (Bids)",
  "CASsL5nemAKjD6qCPKXWmfWK1WHQXARVoBUynmTnfgS2": "Serum: SRM / USDC - Order Book (Asks)",
  "7pzWoBvBdQkF8SLA2PjYNX2aPddPCSnqQ8LTefvQAaq2": "Serum: SRM / USDC - Base Asset Vault",
  "Gge5vGgh15dWeJPJSQyErqiTVffUNkctteAoT4tHjRFB": "Serum: SRM / USDC - Quote Currency Vault",
  "AZVmspyVcUv78HX9PtMFr5guSswotsV6Afsr7Vaifz2P": "Serum: SRM / USDC - Vault Owner",

  "HARFLhSq8nECZk4DVFKvzqXMNMA9a3hjvridGMFizeLa": "Serum: SRM / USDT",
  "3p86S1RzNo7sCVBD3awQ9M88C9bHdgCeDirU2bDv7P3n": "Serum: SRM / USDT - Request Queue",
  "yaJV7SqjmDyQXkpEtyxF4R1k3UdVJtdKse7RHB3hKLE": "Serum: SRM / USDT - Event Queue",
  "4KoZ6w5hbUGUgBJyAvUZAtHx2MCDyoGrFXZyAXrpu3ep": "Serum: SRM / USDT - Order Book (Bids)",
  "CS3k6C8gWUupTBeEeh8AyK8fHN16Yj9zXaqLPh3Q4Ti8": "Serum: SRM / USDT - Order Book (Asks)",
  "8u5NUxd5ShNe5LNv42MXqJ7uuWgAigGe9b5DueLttTh7": "Serum: SRM / USDT - Base Asset Vault",
  "FvTgrXUUD2KiLBybdPa3zG5tJJuCD3qyrJsennNRc58W": "Serum: SRM / USDT - Quote Currency Vault",
  "7SjKY8GoSVTNZa26gNyMtgdUMEKYcxNPtbGVFTmiFbhS": "Serum: SRM / USDT - Vault Owner",

  "FZqrBXz7ADGsmDf1TM9YgysPUfvtG8rJiNUrqDpHc9Au": "Serum: FTC / USDC",
  "2spxPVwPaMWruDHiFfwbchwWG6Lx2soxaCGhjK6DFL1a": "Serum: FTC / USDC - Request Queue",
  "9YbacjStsjbAL2P2tjn3TEALBABBh2jsroq911bEbmva": "Serum: FTC / USDC - Event Queue",
  "8Vwf6LVGyB5XcxRUPC1S4U2U3GkoEf9x6yTLhwGE611y": "Serum: FTC / USDC - Order Book (Bids)",
  "GjasUQFVeXcaGcZk5qFRfdWpbbEMcAbv2EVSFxpu6EDR": "Serum: FTC / USDC - Order Book (Asks)",
  "6HmxKKjmmRZZqUYTH63whUWWF7vdNQfiKx2BtUrtFB3A": "Serum: FTC / USDC - Base Asset Vault",
  "2mULsMxTia5uAiFi8huEQ3zeCe8UBXRdnVaQYybnsJ6L": "Serum: FTC / USDC - Quote Currency Vault",
  "G4FCN83ZSRh4baXDY8eNLhk8reWxKZke3dnDBPmev1Bk": "Serum: FTC / USDC - Vault Owner",

  "DHDdghmkBhEpReno3tbzBPtsxCt6P3KrMzZvxavTktJt": "Serum: FTT / USDT",
  "6cXxUcn7yAkgEWAHqXDav7zeEkbGRAC3cR8VpgzD8kxy": "Serum: FTT / USDT - Request Queue",
  "GmE8DSdDkEJJfzABt7DRYB2DUrPKSLgjzBMsgHjpKmZX": "Serum: FTT / USDT - Event Queue",
  "DiBDJzU91rSGqrTjnVtQgnsJ3dGYQ2TYwspwxsMLzZV4": "Serum: FTT / USDT - Order Book (Bids)",
  "B2qVWSfy4HFK9tGATeYWFbMZ6fi54DEiMCh5bDGmy9n4": "Serum: FTT / USDT - Order Book (Asks)",
  "6xKUQy2Ao4KVvyPns88AwPXchemb9EY4YB3r8gczg6gu": "Serum: FTT / USDT - Base Asset Vault",
  "DEixXfs1PJ4q8VcNYJas2JFDdgFnKMS5mEj9WKP9Kn1J": "Serum: FTT / USDT - Quote Currency Vault",
  "F3Wh4ZAEkR8xrdXwzrADLotvJmYjUiQNwWVvm91mAAa8": "Serum: FTT / USDT - Vault Owner",

  "FJg9FUtbN3fg3YFbMCFiZKjGh5Bn4gtzxZmtxFzmz9kT": "Serum: YFI / USDC",
  "13mTW8D1yoyXMVyCKtmVBtH1sSnHBUWCjfjNJp6CEtDf": "Serum: YFI / USDC - Request Queue",
  "EzZkAWQ97k5D1Augf1bopK7zf9CwJZ9V6eCua6yNGnNj": "Serum: YFI / USDC - Event Queue",
  "5ULcF7rHaMFqmGWBYxcsFGSpo2SfWYzJ4TcMAYWrMG3S": "Serum: YFI / USDC - Order Book (Bids)",
  "78XjqnsSY9CWibWCiAumfx8oSL7yCjoWA1DuuvN4TfUr": "Serum: YFI / USDC - Order Book (Asks)",
  "7j3UcJ77HqA7MTpBNYniBtkDs2F5EgxaLv4oRhK1t63Z": "Serum: YFI / USDC - Base Asset Vault",
  "AbMjvkKRRa5SSwR1MHrSL5inxuybDDMfL3F5RJuMyaHL": "Serum: YFI / USDC - Quote Currency Vault",
  "AnuZK381G6gSbfMsWykkD33WwS4BENfL2rqciQhfbN9L": "Serum: YFI / USDC - Vault Owner",

  "5zu5bTZZvqESAAgFsr12CUMxdQvMrvU9CgvC1GW8vJdf": "Serum: YFI / USDT",
  "HRhgyAnVKX5waJWyC9Yd8jiBnEtwxzjKwk2feSVKrWs3": "Serum: YFI / USDT - Request Queue",
  "DZ9rDKDzimKwTCspqePzH5768rqxkjEkDdhHT46ZqSiJ": "Serum: YFI / USDT - Event Queue",
  "M6cbtnTtDKeehBpie5edWNLX9C3CNFzZA7HBhjevXiJ": "Serum: YFI / USDT - Order Book (Bids)",
  "C7XehQ7R7R8CrxJkJFhoRtsV4SJyKhYoawqadxxYYs3F": "Serum: YFI / USDT - Order Book (Asks)",
  "9gFKq8bvkCYwqoLQj6ZXckggS2BEb7Jj6mnUMPxagDVj": "Serum: YFI / USDT - Base Asset Vault",
  "8GF7m51UzcvRWwcRcXRpttetB7PwfHdwn9fqE9PCLk6F": "Serum: YFI / USDT - Quote Currency Vault",
  "Ci5BrxgxTB2nfmsBz5REpDaChJTZk7FhFTBb8jFbfJp5": "Serum: YFI / USDT - Vault Owner",

  "7GZ59DMgJ7D6dfoJTpszPayTRyua9jwcaGJXaRMMF1my": "Serum: LINK / USDC",
  "8wTo4Tn3QfzYwyjoofutKU4CE8i98Zj8AJrSa2BL4vso": "Serum: LINK / USDC - Request Queue",
  "4BTeKadBCoTaCrxwSYj36FNiutZFvRMi8bvuPqZA9pKW": "Serum: LINK / USDC - Event Queue",
  "GoaFC6qRq7MNy1suxvPSNYeoioN32kad17qbdqqWEnTe": "Serum: LINK / USDC - Order Book (Bids)",
  "9cLbP6pEwEXDttbgTrPqYzxyXnHFygj29DT1EdtrQFfS": "Serum: LINK / USDC - Order Book (Asks)",
  "2vXebfXSBhQ1aEV5Nnv4j3eo1RJVuJP7CoWmHNdvPvhk": "Serum: LINK / USDC - Base Asset Vault",
  "BnzYkmNRVZ6Q71mvzLwYqkU5B4S2hUdqm1GyU2DfQJEC": "Serum: LINK / USDC - Quote Currency Vault",
  "Fpg7XoRAfvrxaVSHmuUS8HRfGzBjRmMoWvRDHoW6G3zV": "Serum: LINK / USDC - Vault Owner",

  "F5xschQBMpu1gD2q1babYEAVJHR1buj1YazLiXyQNqSW": "Serum: LINK / USDT",
  "Ga48n8R13mW4jFnUXmejVa4tPpR8nAo3mYKG95qZEUZV": "Serum: LINK / USDT - Request Queue",
  "H7ZHuTaMHswJWbJxsCgtV4ar28Kjty2hB1DbZVT3icjB": "Serum: LINK / USDT - Event Queue",
  "HAx1rwxYapD4CPm9G3H2hq1bufykJg6LksZjU39HwHov": "Serum: LINK / USDT - Order Book (Bids)",
  "7sjAfkzD9xCU68dmvCtt5mEFBaFzqF8GrYjKdUDhLnst": "Serum: LINK / USDT - Order Book (Asks)",
  "5vNPSbGTMUzKPQtDdDBGUCeCip9uD8igj4Erfdzz7YdU": "Serum: LINK / USDT - Base Asset Vault",
  "76jYTFWR1qjaxNeNvLmHuLeitBEdpQ9P7QPBa4pMp5ve": "Serum: LINK / USDT - Quote Currency Vault",
  "BeZc9vWGyNJ5g6gG8JoRSkvY5oJgyjV7ErLajfVDJ3FA": "Serum: LINK / USDT - Vault Owner",

  "4eM8iy2k7VXec5VxRk8xFRhHw5Cn67m5FjNzCEegWibm": "Serum: XRP / USDC",
  "56RPf4XMR1wLhPKnxgr1cko9gpDWwE7i8w5owWLW4qNT": "Serum: XRP / USDC - Request Queue",
  "49oPGPoTexM5CLzrbrMNiNbrrRjhBfLvEPTZRzsPYL3g": "Serum: XRP / USDC - Event Queue",
  "66oziZDCXpsCJkYHcxucmEp6bCPaRJUPeeAjSwnudkhb": "Serum: XRP / USDC - Order Book (Bids)",
  "9cy917jmyRQqQCocgtnPv5gjiUMx4x4npUPF2CP9bHxP": "Serum: XRP / USDC - Order Book (Asks)",
  "9Sfk6Hk9V8FR3kxYNhNt1STKAtWuXifQGKxUJ9g3Np4b": "Serum: XRP / USDC - Base Asset Vault",
  "8gDM85fdTxEWYVbBPDQFh1d5gVRTDQ4vhpCai3UTEdS7": "Serum: XRP / USDC - Quote Currency Vault",
  "5LaKtnftuks8DB8aJGuHrMnY95KJcUhSpJ77pRi5mvGP": "Serum: XRP / USDC - Vault Owner",

  "H5BtazuKhHtZCZjFTZSRd4W3QZRaRgq6JcQCBLbjkj1o": "Serum: XRP / USDT",
  "6tYvWamhBADnQ3rv7BvAUMyKgcJ2CgtNGSw79REXnsc6": "Serum: XRP / USDT - Request Queue",
  "eKr9MEvo1Nv1CMYTjUE3cw1qk3infy5VdtaCZEFkZzn": "Serum: XRP / USDT - Event Queue",
  "HrmGTNSsAoe7cUdtZPqzZmnTfnS9WvNwwqiAYXi8ZwAW": "Serum: XRP / USDT - Order Book (Bids)",
  "BVejg9wNCCai4n2EgTpkiPev3gvS15f1AoKTmK9b2pNs": "Serum: XRP / USDT - Order Book (Asks)",
  "2SmRPibwnar9Gd9byx5nK6D8ikAcEUQn7ZC6yqjTBkDg": "Serum: XRP / USDT - Base Asset Vault",
  "3jjzgJWeFk3J57JvSJ6P22GdyQFkBTbTLsWB6eSNf6WJ": "Serum: XRP / USDT - Quote Currency Vault",
  "GJKaUk5NCHL4LTpP1Yn57qzTdCtjd3mSCyvXdktF4uPx": "Serum: XRP / USDT - Vault Owner",

  "BGjv1z7GLAHQH9F9Xd6a3idz5JUo1tka8rsLizPvPq5Z": "Serum: SRM / USDC",
  "E6EQQpB7nQwbzggSCUUPhjE8C9TGSw5dguQuQPMgr5Dt": "Serum: SRM / USDC - Request Queue",
  "8yG8L7hcUtnboi4RcGQ1SBz9njeXzPvJUrmYrysFr1rT": "Serum: SRM / USDC - Event Queue",
  "FjPSrcboahL3jGe4GVZDPURQWE8dL3TWVFHNznK6qaXi": "Serum: SRM / USDC - Order Book (Bids)",
  "Bh5JCiV2uMsNLDiy5oKJ23YmCPwnmeGQ34kGHUF5dhgu": "Serum: SRM / USDC - Order Book (Asks)",
  "8q42g61uTZ9bH9RFdWJyceBTcsRJyaeehzUtSyYzMbj2": "Serum: SRM / USDC - Base Asset Vault",
  "2Rn3quSFg8cKTcHwadmEXoR2dcjqhsF3s3AJXZUDKMJb": "Serum: SRM / USDC - Quote Currency Vault",
  "BeiNALSUcT23SqwnGKxJ6XUG1jeFcCozKd3SBKVgDRBN": "Serum: SRM / USDC - Vault Owner",

  "9kJ8YCHZSiqXgfFyT9LoNVFEv8og3C5oN1pPCwmYRgCz": "Serum: SRM / USDT",
  "9DsKwjwP1wkg2UaYXESQtbbTYJ6Q888gfuoa1yAuzMAp": "Serum: SRM / USDT - Request Queue",
  "BKhdZHfXaVQ1TjG8mNvifJiMxazjRpWcznjCbtxt3YNv": "Serum: SRM / USDT - Event Queue",
  "4iy4REdaS3AEWC4fGZmxP121qKTZ659EeKYyP5uB66qj": "Serum: SRM / USDT - Order Book (Bids)",
  "2BmuY8oAZVUSn9F2r8JGcLcJN8VHc5QFecub7XBx234W": "Serum: SRM / USDT - Order Book (Asks)",
  "HZ1aXc9KVfGGS9mD2mzxHh5jeJSvQrx1dz4zGWBaGmkE": "Serum: SRM / USDT - Base Asset Vault",
  "FEyMWn3KnWVEMeJr8q7nQxfQu3jq15MchkhLq94kEoVh": "Serum: SRM / USDT - Quote Currency Vault",
  "9sBxruSjQ97z8k42RXAr2grx9S7Fsk2yisbvZE9cNfjG": "Serum: SRM / USDT - Vault Owner",
};

function get(address: string, cluster: Cluster): string | undefined {
  if (cluster === Cluster.MainnetBeta) return MARKET_REGISTRY[address];
}

export const SerumMarketRegistry = {
  get,
};
