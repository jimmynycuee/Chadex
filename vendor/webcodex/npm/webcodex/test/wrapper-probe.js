"use strict";
process.env.WEBCODEX_TEST_EXIT = "23";
const { runNative } = require("../bin/wrapper");
runNative({ target: process.argv[2], argv: process.argv.slice(3) });
