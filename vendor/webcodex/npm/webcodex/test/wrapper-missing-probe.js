"use strict";
const { runNative } = require("../bin/wrapper");
runNative({ target: "/definitely/missing/webcodex" });
