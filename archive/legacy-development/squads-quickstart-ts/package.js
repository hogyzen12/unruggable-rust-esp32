{
    "scripts": {
      "test": "npx mocha -r ts-node/register 'main.ts' --timeout 10000"
    },
    "dependencies": {
      "@solana/web3.js": "^1.73.0",
      "@sqds/multisig": "^2.1.3"
    },
    "devDependencies": {
      "@types/chai": "^4.3.3",
      "@types/mocha": "^10.0.6",
      "chai": "^4.3.6",
      "mocha": "^10.3.0",
      "ts-mocha": "^10.0.0",
      "typescript": "^4.8.3"
    }
  }