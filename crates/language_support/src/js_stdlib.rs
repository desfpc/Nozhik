//! ECMAScript built-ins, language constructs and the browser platform used by
//! completion and hover. Queried nodes are `HTMLElement`, so `querySelector`,
//! `querySelectorAll` and `createElement` expose the fields used in page scripts.
//! WebGL, WebRTC, Web Audio, service-worker internals and a separate interface
//! for every HTML tag are not included. `data-*` keys and the long tail of CSS
//! property names stay dynamic.

use crate::{
    doc_provider::{DefinitionTarget, EntityKind, LanguageDocProvider, SymbolDocInfo},
    lsp_types,
};

/// One method or data property of a built-in.
#[derive(Clone, Copy)]
pub struct Member {
    pub name: &'static str,
    pub signature: &'static str,
    pub documentation: &'static str,
    /// Empty when the result type is not a known built-in.
    pub returns: &'static str,
    pub property: bool,
}

/// A built-in object, constructor, or namespace.
#[derive(Clone, Copy)]
pub struct Type {
    pub name: &'static str,
    pub documentation: &'static str,
    pub constructable: bool,
    /// `Math`, `JSON`, `console`: the binding's members are the static list.
    pub namespace: bool,
    pub instance: &'static [Member],
    pub static_members: &'static [Member],
}

/// A global object binding such as `document`.
#[derive(Clone, Copy)]
pub struct GlobalBinding {
    /// Binding name.
    pub name: &'static str,
    /// Instance type used for member completion.
    pub type_name: &'static str,
    /// Short hover text.
    pub documentation: &'static str,
}

/// A global function such as `parseInt`.
#[derive(Clone, Copy)]
pub struct GlobalFn {
    pub name: &'static str,
    pub signature: &'static str,
    pub documentation: &'static str,
    pub returns: &'static str,
}

/// A completion snippet for a statement or keyword.
#[derive(Clone, Copy)]
pub struct Construct {
    pub name: &'static str,
    pub insert: &'static str,
    /// Byte offset inside `insert` where the caret should land. `None` leaves it at the end.
    pub caret: Option<usize>,
    pub documentation: &'static str,
    pub url: &'static str,
}

macro_rules! method {
    ($name:literal, $sig:literal, $ret:literal) => {
        Member {
            name: $name,
            signature: $sig,
            documentation: "",
            returns: $ret,
            property: false,
        }
    };
}
macro_rules! prop {
    ($name:literal, $sig:literal, $ret:literal) => {
        Member {
            name: $name,
            signature: $sig,
            documentation: "",
            returns: $ret,
            property: true,
        }
    };
}

const OBJECT_INSTANCE: &[Member] = &[
    method!("hasOwnProperty", "hasOwnProperty(property)", "Boolean"),
    method!("isPrototypeOf", "isPrototypeOf(object)", "Boolean"),
    method!(
        "propertyIsEnumerable",
        "propertyIsEnumerable(property)",
        "Boolean"
    ),
    method!("toLocaleString", "toLocaleString()", "String"),
    method!("toString", "toString()", "String"),
    method!("valueOf", "valueOf()", ""),
];

const OBJECT_STATIC: &[Member] = &[
    method!("assign", "Object.assign(target, ...sources)", "Object"),
    method!("create", "Object.create(prototype)", "Object"),
    method!(
        "defineProperties",
        "Object.defineProperties(object, properties)",
        "Object"
    ),
    method!(
        "defineProperty",
        "Object.defineProperty(object, property, descriptor)",
        "Object"
    ),
    method!("entries", "Object.entries(object)", "Array"),
    method!("freeze", "Object.freeze(object)", "Object"),
    method!("fromEntries", "Object.fromEntries(iterable)", "Object"),
    method!(
        "getOwnPropertyDescriptor",
        "Object.getOwnPropertyDescriptor(object, property)",
        "Object"
    ),
    method!(
        "getOwnPropertyDescriptors",
        "Object.getOwnPropertyDescriptors(object)",
        "Object"
    ),
    method!(
        "getOwnPropertyNames",
        "Object.getOwnPropertyNames(object)",
        "Array"
    ),
    method!(
        "getOwnPropertySymbols",
        "Object.getOwnPropertySymbols(object)",
        "Array"
    ),
    method!("getPrototypeOf", "Object.getPrototypeOf(object)", "Object"),
    method!("groupBy", "Object.groupBy(items, callback)", "Object"),
    method!("hasOwn", "Object.hasOwn(object, property)", "Boolean"),
    method!("is", "Object.is(value1, value2)", "Boolean"),
    method!("isExtensible", "Object.isExtensible(object)", "Boolean"),
    method!("isFrozen", "Object.isFrozen(object)", "Boolean"),
    method!("isSealed", "Object.isSealed(object)", "Boolean"),
    method!("keys", "Object.keys(object)", "Array"),
    method!(
        "preventExtensions",
        "Object.preventExtensions(object)",
        "Object"
    ),
    method!("seal", "Object.seal(object)", "Object"),
    method!(
        "setPrototypeOf",
        "Object.setPrototypeOf(object, prototype)",
        "Object"
    ),
    method!("values", "Object.values(object)", "Array"),
];

const ARRAY_INSTANCE: &[Member] = &[
    method!("at", "at(index)", ""),
    method!("concat", "concat(...items)", "Array"),
    method!("copyWithin", "copyWithin(target, start, end)", "Array"),
    method!("entries", "entries()", "Array"),
    method!("every", "every(callback)", "Boolean"),
    method!("fill", "fill(value, start, end)", "Array"),
    method!("filter", "filter(callback)", "Array"),
    method!("find", "find(callback)", ""),
    method!("findIndex", "findIndex(callback)", "Number"),
    method!("findLast", "findLast(callback)", ""),
    method!("findLastIndex", "findLastIndex(callback)", "Number"),
    method!("flat", "flat(depth)", "Array"),
    method!("flatMap", "flatMap(callback)", "Array"),
    method!("forEach", "forEach(callback)", ""),
    method!("includes", "includes(searchElement, fromIndex)", "Boolean"),
    method!("indexOf", "indexOf(searchElement, fromIndex)", "Number"),
    method!("join", "join(separator)", "String"),
    method!("keys", "keys()", "Array"),
    method!(
        "lastIndexOf",
        "lastIndexOf(searchElement, fromIndex)",
        "Number"
    ),
    prop!("length", "length", "Number"),
    method!("map", "map(callback)", "Array"),
    method!("pop", "pop()", ""),
    method!("push", "push(...items)", "Number"),
    method!("reduce", "reduce(callback, initialValue)", ""),
    method!("reduceRight", "reduceRight(callback, initialValue)", ""),
    method!("reverse", "reverse()", "Array"),
    method!("shift", "shift()", ""),
    method!("slice", "slice(start, end)", "Array"),
    method!("some", "some(callback)", "Boolean"),
    method!("sort", "sort(compare)", "Array"),
    method!("splice", "splice(start, deleteCount, ...items)", "Array"),
    method!("toReversed", "toReversed()", "Array"),
    method!("toSorted", "toSorted(compare)", "Array"),
    method!(
        "toSpliced",
        "toSpliced(start, deleteCount, ...items)",
        "Array"
    ),
    method!("toLocaleString", "toLocaleString()", "String"),
    method!("toString", "toString()", "String"),
    method!("unshift", "unshift(...items)", "Number"),
    method!("values", "values()", "Array"),
    method!("with", "with(index, value)", "Array"),
];

const ARRAY_STATIC: &[Member] = &[
    method!("from", "Array.from(items, mapFn)", "Array"),
    method!("fromAsync", "Array.fromAsync(items, mapFn)", "Promise"),
    method!("isArray", "Array.isArray(value)", "Boolean"),
    method!("of", "Array.of(...items)", "Array"),
];

const STRING_INSTANCE: &[Member] = &[
    method!("at", "at(index)", "String"),
    method!("charAt", "charAt(index)", "String"),
    method!("charCodeAt", "charCodeAt(index)", "Number"),
    method!("codePointAt", "codePointAt(index)", "Number"),
    method!("concat", "concat(...strings)", "String"),
    method!("endsWith", "endsWith(search, end)", "Boolean"),
    method!("includes", "includes(search, position)", "Boolean"),
    method!("indexOf", "indexOf(search, position)", "Number"),
    method!("lastIndexOf", "lastIndexOf(search, position)", "Number"),
    prop!("length", "length", "Number"),
    method!("localeCompare", "localeCompare(compare)", "Number"),
    method!("match", "match(regexp)", "Array"),
    method!("matchAll", "matchAll(regexp)", "Array"),
    method!("normalize", "normalize(form)", "String"),
    method!("padEnd", "padEnd(targetLength, pad)", "String"),
    method!("padStart", "padStart(targetLength, pad)", "String"),
    method!("repeat", "repeat(count)", "String"),
    method!("replace", "replace(search, replacement)", "String"),
    method!("replaceAll", "replaceAll(search, replacement)", "String"),
    method!("search", "search(regexp)", "Number"),
    method!("slice", "slice(start, end)", "String"),
    method!("split", "split(separator, limit)", "Array"),
    method!("startsWith", "startsWith(search, position)", "Boolean"),
    method!("substring", "substring(start, end)", "String"),
    method!("toLocaleLowerCase", "toLocaleLowerCase()", "String"),
    method!("toLocaleUpperCase", "toLocaleUpperCase()", "String"),
    method!("toLowerCase", "toLowerCase()", "String"),
    method!("toString", "toString()", "String"),
    method!("toUpperCase", "toUpperCase()", "String"),
    method!("isWellFormed", "isWellFormed()", "Boolean"),
    method!("toWellFormed", "toWellFormed()", "String"),
    method!("trim", "trim()", "String"),
    method!("trimEnd", "trimEnd()", "String"),
    method!("trimStart", "trimStart()", "String"),
    method!("valueOf", "valueOf()", "String"),
];

const STRING_STATIC: &[Member] = &[
    method!("fromCharCode", "String.fromCharCode(...codes)", "String"),
    method!("fromCodePoint", "String.fromCodePoint(...codes)", "String"),
    method!("raw", "String.raw(template, ...substitutions)", "String"),
];

const NUMBER_INSTANCE: &[Member] = &[
    method!("toExponential", "toExponential(fractionDigits)", "String"),
    method!("toFixed", "toFixed(digits)", "String"),
    method!("toLocaleString", "toLocaleString()", "String"),
    method!("toPrecision", "toPrecision(precision)", "String"),
    method!("toString", "toString(radix)", "String"),
    method!("valueOf", "valueOf()", "Number"),
];

const NUMBER_STATIC: &[Member] = &[
    prop!("EPSILON", "Number.EPSILON", "Number"),
    prop!("MAX_SAFE_INTEGER", "Number.MAX_SAFE_INTEGER", "Number"),
    prop!("MAX_VALUE", "Number.MAX_VALUE", "Number"),
    prop!("MIN_SAFE_INTEGER", "Number.MIN_SAFE_INTEGER", "Number"),
    prop!("MIN_VALUE", "Number.MIN_VALUE", "Number"),
    prop!("NaN", "Number.NaN", "Number"),
    prop!("NEGATIVE_INFINITY", "Number.NEGATIVE_INFINITY", "Number"),
    prop!("POSITIVE_INFINITY", "Number.POSITIVE_INFINITY", "Number"),
    method!("isFinite", "Number.isFinite(value)", "Boolean"),
    method!("isInteger", "Number.isInteger(value)", "Boolean"),
    method!("isNaN", "Number.isNaN(value)", "Boolean"),
    method!("isSafeInteger", "Number.isSafeInteger(value)", "Boolean"),
    method!("parseFloat", "Number.parseFloat(value)", "Number"),
    method!("parseInt", "Number.parseInt(value, radix)", "Number"),
];

const FUNCTION_INSTANCE: &[Member] = &[
    method!("apply", "apply(thisArg, args)", ""),
    method!("bind", "bind(thisArg, ...args)", "Function"),
    method!("call", "call(thisArg, ...args)", ""),
    prop!("length", "length", "Number"),
    prop!("name", "name", "String"),
    method!("toString", "toString()", "String"),
];

const BOOLEAN_INSTANCE: &[Member] = &[
    method!("toString", "toString()", "String"),
    method!("valueOf", "valueOf()", "Boolean"),
];

const SYMBOL_STATIC: &[Member] = &[
    prop!("asyncIterator", "Symbol.asyncIterator", "Symbol"),
    method!("for", "Symbol.for(key)", "Symbol"),
    prop!("hasInstance", "Symbol.hasInstance", "Symbol"),
    prop!("isConcatSpreadable", "Symbol.isConcatSpreadable", "Symbol"),
    prop!("iterator", "Symbol.iterator", "Symbol"),
    method!("keyFor", "Symbol.keyFor(symbol)", "String"),
    prop!("match", "Symbol.match", "Symbol"),
    prop!("matchAll", "Symbol.matchAll", "Symbol"),
    prop!("replace", "Symbol.replace", "Symbol"),
    prop!("search", "Symbol.search", "Symbol"),
    prop!("species", "Symbol.species", "Symbol"),
    prop!("split", "Symbol.split", "Symbol"),
    prop!("toPrimitive", "Symbol.toPrimitive", "Symbol"),
    prop!("toStringTag", "Symbol.toStringTag", "Symbol"),
    prop!("unscopables", "Symbol.unscopables", "Symbol"),
];

const SYMBOL_INSTANCE: &[Member] = &[
    prop!("description", "description", "String"),
    method!("toString", "toString()", "String"),
    method!("valueOf", "valueOf()", "Symbol"),
];

const BIGINT_STATIC: &[Member] = &[
    method!("asIntN", "BigInt.asIntN(bits, bigint)", "BigInt"),
    method!("asUintN", "BigInt.asUintN(bits, bigint)", "BigInt"),
];

const BIGINT_INSTANCE: &[Member] = &[
    method!("toLocaleString", "toLocaleString()", "String"),
    method!("toString", "toString(radix)", "String"),
    method!("valueOf", "valueOf()", "BigInt"),
];

const DATE_INSTANCE: &[Member] = &[
    method!("getDate", "getDate()", "Number"),
    method!("getDay", "getDay()", "Number"),
    method!("getFullYear", "getFullYear()", "Number"),
    method!("getHours", "getHours()", "Number"),
    method!("getMilliseconds", "getMilliseconds()", "Number"),
    method!("getMinutes", "getMinutes()", "Number"),
    method!("getMonth", "getMonth()", "Number"),
    method!("getSeconds", "getSeconds()", "Number"),
    method!("getTime", "getTime()", "Number"),
    method!("getTimezoneOffset", "getTimezoneOffset()", "Number"),
    method!("getUTCDate", "getUTCDate()", "Number"),
    method!("getUTCDay", "getUTCDay()", "Number"),
    method!("getUTCFullYear", "getUTCFullYear()", "Number"),
    method!("getUTCHours", "getUTCHours()", "Number"),
    method!("getUTCMilliseconds", "getUTCMilliseconds()", "Number"),
    method!("getUTCMinutes", "getUTCMinutes()", "Number"),
    method!("getUTCMonth", "getUTCMonth()", "Number"),
    method!("getUTCSeconds", "getUTCSeconds()", "Number"),
    method!("setDate", "setDate(day)", "Number"),
    method!("setFullYear", "setFullYear(year, month, day)", "Number"),
    method!(
        "setHours",
        "setHours(hours, minutes, seconds, ms)",
        "Number"
    ),
    method!("setMilliseconds", "setMilliseconds(ms)", "Number"),
    method!("setMinutes", "setMinutes(minutes, seconds, ms)", "Number"),
    method!("setMonth", "setMonth(month, day)", "Number"),
    method!("setSeconds", "setSeconds(seconds, ms)", "Number"),
    method!("setTime", "setTime(time)", "Number"),
    method!("setUTCDate", "setUTCDate(day)", "Number"),
    method!(
        "setUTCFullYear",
        "setUTCFullYear(year, month, day)",
        "Number"
    ),
    method!(
        "setUTCHours",
        "setUTCHours(hours, minutes, seconds, ms)",
        "Number"
    ),
    method!("setUTCMilliseconds", "setUTCMilliseconds(ms)", "Number"),
    method!(
        "setUTCMinutes",
        "setUTCMinutes(minutes, seconds, ms)",
        "Number"
    ),
    method!("setUTCMonth", "setUTCMonth(month, day)", "Number"),
    method!("setUTCSeconds", "setUTCSeconds(seconds, ms)", "Number"),
    method!("toDateString", "toDateString()", "String"),
    method!("toISOString", "toISOString()", "String"),
    method!("toJSON", "toJSON()", "String"),
    method!("toLocaleDateString", "toLocaleDateString()", "String"),
    method!("toLocaleString", "toLocaleString()", "String"),
    method!("toLocaleTimeString", "toLocaleTimeString()", "String"),
    method!("toString", "toString()", "String"),
    method!("toTimeString", "toTimeString()", "String"),
    method!("toUTCString", "toUTCString()", "String"),
    method!("valueOf", "valueOf()", "Number"),
];

const DATE_STATIC: &[Member] = &[
    method!("now", "Date.now()", "Number"),
    method!("parse", "Date.parse(value)", "Number"),
    method!(
        "UTC",
        "Date.UTC(year, month, day, hours, minutes, seconds, ms)",
        "Number"
    ),
];

const REGEXP_INSTANCE: &[Member] = &[
    prop!("dotAll", "dotAll", "Boolean"),
    method!("exec", "exec(string)", "Array"),
    prop!("flags", "flags", "String"),
    prop!("global", "global", "Boolean"),
    prop!("hasIndices", "hasIndices", "Boolean"),
    prop!("ignoreCase", "ignoreCase", "Boolean"),
    prop!("lastIndex", "lastIndex", "Number"),
    prop!("multiline", "multiline", "Boolean"),
    prop!("source", "source", "String"),
    prop!("sticky", "sticky", "Boolean"),
    method!("test", "test(string)", "Boolean"),
    method!("toString", "toString()", "String"),
    prop!("unicode", "unicode", "Boolean"),
    prop!("unicodeSets", "unicodeSets", "Boolean"),
];

const REGEXP_STATIC: &[Member] = &[method!("escape", "RegExp.escape(string)", "String")];

const ERROR_STATIC: &[Member] = &[method!("isError", "Error.isError(value)", "Boolean")];

const ERROR_INSTANCE: &[Member] = &[
    prop!("cause", "cause", ""),
    prop!("message", "message", "String"),
    prop!("name", "name", "String"),
    prop!("stack", "stack", "String"),
    method!("toString", "toString()", "String"),
];

const AGGREGATE_ERROR: &[Member] = &[prop!("errors", "errors", "Array")];

const MAP_INSTANCE: &[Member] = &[
    method!("clear", "clear()", ""),
    method!("delete", "delete(key)", "Boolean"),
    method!("entries", "entries()", "Array"),
    method!("forEach", "forEach(callback)", ""),
    method!("get", "get(key)", ""),
    method!("has", "has(key)", "Boolean"),
    method!("keys", "keys()", "Array"),
    method!("set", "set(key, value)", "Map"),
    prop!("size", "size", "Number"),
    method!("values", "values()", "Array"),
];

const MAP_STATIC: &[Member] = &[method!("groupBy", "Map.groupBy(items, callback)", "Map")];

const SET_INSTANCE: &[Member] = &[
    method!("add", "add(value)", "Set"),
    method!("clear", "clear()", ""),
    method!("delete", "delete(value)", "Boolean"),
    method!("difference", "difference(other)", "Set"),
    method!("entries", "entries()", "Array"),
    method!("forEach", "forEach(callback)", ""),
    method!("has", "has(value)", "Boolean"),
    method!("intersection", "intersection(other)", "Set"),
    method!("isDisjointFrom", "isDisjointFrom(other)", "Boolean"),
    method!("isSubsetOf", "isSubsetOf(other)", "Boolean"),
    method!("isSupersetOf", "isSupersetOf(other)", "Boolean"),
    method!("keys", "keys()", "Array"),
    prop!("size", "size", "Number"),
    method!("symmetricDifference", "symmetricDifference(other)", "Set"),
    method!("union", "union(other)", "Set"),
    method!("values", "values()", "Array"),
];

const WEAK_MAP: &[Member] = &[
    method!("delete", "delete(key)", "Boolean"),
    method!("get", "get(key)", ""),
    method!("has", "has(key)", "Boolean"),
    method!("set", "set(key, value)", "WeakMap"),
];

const WEAK_SET: &[Member] = &[
    method!("add", "add(value)", "WeakSet"),
    method!("delete", "delete(value)", "Boolean"),
    method!("has", "has(value)", "Boolean"),
];

const PROMISE_INSTANCE: &[Member] = &[
    method!("catch", "catch(onRejected)", "Promise"),
    method!("finally", "finally(onFinally)", "Promise"),
    method!("then", "then(onFulfilled, onRejected)", "Promise"),
];

const PROMISE_STATIC: &[Member] = &[
    method!("all", "Promise.all(iterable)", "Promise"),
    method!("allSettled", "Promise.allSettled(iterable)", "Promise"),
    method!("any", "Promise.any(iterable)", "Promise"),
    method!("race", "Promise.race(iterable)", "Promise"),
    method!("reject", "Promise.reject(reason)", "Promise"),
    method!("resolve", "Promise.resolve(value)", "Promise"),
    method!("try", "Promise.try(callback)", "Promise"),
    method!("withResolvers", "Promise.withResolvers()", "Object"),
];

const JSON_STATIC: &[Member] = &[
    method!("isRawJSON", "JSON.isRawJSON(value)", "Boolean"),
    method!("parse", "JSON.parse(text, reviver)", ""),
    method!("rawJSON", "JSON.rawJSON(string)", "Object"),
    method!(
        "stringify",
        "JSON.stringify(value, replacer, space)",
        "String"
    ),
];

const MATH_STATIC: &[Member] = &[
    prop!("E", "Math.E", "Number"),
    prop!("LN10", "Math.LN10", "Number"),
    prop!("LN2", "Math.LN2", "Number"),
    prop!("LOG10E", "Math.LOG10E", "Number"),
    prop!("LOG2E", "Math.LOG2E", "Number"),
    prop!("PI", "Math.PI", "Number"),
    prop!("SQRT1_2", "Math.SQRT1_2", "Number"),
    prop!("SQRT2", "Math.SQRT2", "Number"),
    method!("abs", "Math.abs(x)", "Number"),
    method!("acos", "Math.acos(x)", "Number"),
    method!("acosh", "Math.acosh(x)", "Number"),
    method!("asin", "Math.asin(x)", "Number"),
    method!("asinh", "Math.asinh(x)", "Number"),
    method!("atan", "Math.atan(x)", "Number"),
    method!("atan2", "Math.atan2(y, x)", "Number"),
    method!("atanh", "Math.atanh(x)", "Number"),
    method!("cbrt", "Math.cbrt(x)", "Number"),
    method!("ceil", "Math.ceil(x)", "Number"),
    method!("clz32", "Math.clz32(x)", "Number"),
    method!("cos", "Math.cos(x)", "Number"),
    method!("cosh", "Math.cosh(x)", "Number"),
    method!("exp", "Math.exp(x)", "Number"),
    method!("expm1", "Math.expm1(x)", "Number"),
    method!("floor", "Math.floor(x)", "Number"),
    method!("fround", "Math.fround(x)", "Number"),
    method!("hypot", "Math.hypot(...values)", "Number"),
    method!("imul", "Math.imul(a, b)", "Number"),
    method!("log", "Math.log(x)", "Number"),
    method!("log10", "Math.log10(x)", "Number"),
    method!("log1p", "Math.log1p(x)", "Number"),
    method!("log2", "Math.log2(x)", "Number"),
    method!("max", "Math.max(...values)", "Number"),
    method!("min", "Math.min(...values)", "Number"),
    method!("pow", "Math.pow(base, exponent)", "Number"),
    method!("random", "Math.random()", "Number"),
    method!("round", "Math.round(x)", "Number"),
    method!("sign", "Math.sign(x)", "Number"),
    method!("sin", "Math.sin(x)", "Number"),
    method!("sinh", "Math.sinh(x)", "Number"),
    method!("sqrt", "Math.sqrt(x)", "Number"),
    method!("tan", "Math.tan(x)", "Number"),
    method!("tanh", "Math.tanh(x)", "Number"),
    method!("trunc", "Math.trunc(x)", "Number"),
];

const REFLECT_STATIC: &[Member] = &[
    method!("apply", "Reflect.apply(target, thisArg, args)", ""),
    method!(
        "construct",
        "Reflect.construct(target, args, newTarget)",
        "Object"
    ),
    method!(
        "defineProperty",
        "Reflect.defineProperty(target, property, descriptor)",
        "Boolean"
    ),
    method!(
        "deleteProperty",
        "Reflect.deleteProperty(target, property)",
        "Boolean"
    ),
    method!("get", "Reflect.get(target, property, receiver)", ""),
    method!(
        "getOwnPropertyDescriptor",
        "Reflect.getOwnPropertyDescriptor(target, property)",
        "Object"
    ),
    method!("getPrototypeOf", "Reflect.getPrototypeOf(target)", "Object"),
    method!("has", "Reflect.has(target, property)", "Boolean"),
    method!("isExtensible", "Reflect.isExtensible(target)", "Boolean"),
    method!("ownKeys", "Reflect.ownKeys(target)", "Array"),
    method!(
        "preventExtensions",
        "Reflect.preventExtensions(target)",
        "Boolean"
    ),
    method!(
        "set",
        "Reflect.set(target, property, value, receiver)",
        "Boolean"
    ),
    method!(
        "setPrototypeOf",
        "Reflect.setPrototypeOf(target, prototype)",
        "Boolean"
    ),
];

const ATOMICS_STATIC: &[Member] = &[
    method!("add", "Atomics.add(typedArray, index, value)", "Number"),
    method!("and", "Atomics.and(typedArray, index, value)", "Number"),
    method!(
        "compareExchange",
        "Atomics.compareExchange(typedArray, index, expected, replacement)",
        "Number"
    ),
    method!(
        "exchange",
        "Atomics.exchange(typedArray, index, value)",
        "Number"
    ),
    method!("isLockFree", "Atomics.isLockFree(size)", "Boolean"),
    method!("load", "Atomics.load(typedArray, index)", "Number"),
    method!(
        "notify",
        "Atomics.notify(typedArray, index, count)",
        "Number"
    ),
    method!("or", "Atomics.or(typedArray, index, value)", "Number"),
    method!("store", "Atomics.store(typedArray, index, value)", "Number"),
    method!("sub", "Atomics.sub(typedArray, index, value)", "Number"),
    method!(
        "wait",
        "Atomics.wait(typedArray, index, value, timeout)",
        "String"
    ),
    method!(
        "waitAsync",
        "Atomics.waitAsync(typedArray, index, value, timeout)",
        "Object"
    ),
    method!("xor", "Atomics.xor(typedArray, index, value)", "Number"),
];

const INTL_STATIC: &[Member] = &[
    prop!("Collator", "Intl.Collator", "Collator"),
    prop!("DateTimeFormat", "Intl.DateTimeFormat", "DateTimeFormat"),
    prop!("DisplayNames", "Intl.DisplayNames", "DisplayNames"),
    prop!("ListFormat", "Intl.ListFormat", "ListFormat"),
    prop!("Locale", "Intl.Locale", "Locale"),
    prop!("NumberFormat", "Intl.NumberFormat", "NumberFormat"),
    prop!("PluralRules", "Intl.PluralRules", "PluralRules"),
    prop!(
        "RelativeTimeFormat",
        "Intl.RelativeTimeFormat",
        "RelativeTimeFormat"
    ),
    prop!("Segmenter", "Intl.Segmenter", "Segmenter"),
];

const TYPED_INSTANCE: &[Member] = &[
    method!("at", "at(index)", "Number"),
    prop!("buffer", "buffer", "ArrayBuffer"),
    prop!("byteLength", "byteLength", "Number"),
    prop!("byteOffset", "byteOffset", "Number"),
    method!("entries", "entries()", "Array"),
    method!("every", "every(callback)", "Boolean"),
    method!("fill", "fill(value, start, end)", ""),
    method!("filter", "filter(callback)", "Array"),
    method!("find", "find(callback)", "Number"),
    method!("findIndex", "findIndex(callback)", "Number"),
    method!("findLast", "findLast(callback)", "Number"),
    method!("findLastIndex", "findLastIndex(callback)", "Number"),
    method!("forEach", "forEach(callback)", ""),
    method!("includes", "includes(searchElement, fromIndex)", "Boolean"),
    method!("indexOf", "indexOf(searchElement, fromIndex)", "Number"),
    method!("join", "join(separator)", "String"),
    method!("keys", "keys()", "Array"),
    prop!("length", "length", "Number"),
    method!("map", "map(callback)", "Array"),
    method!("reduce", "reduce(callback, initialValue)", ""),
    method!("reduceRight", "reduceRight(callback, initialValue)", ""),
    method!("reverse", "reverse()", ""),
    method!("set", "set(array, offset)", ""),
    method!("slice", "slice(start, end)", "Array"),
    method!("some", "some(callback)", "Boolean"),
    method!("sort", "sort(compare)", ""),
    method!("subarray", "subarray(begin, end)", "Array"),
    method!("toReversed", "toReversed()", "Array"),
    method!("toSorted", "toSorted(compare)", "Array"),
    method!("values", "values()", "Array"),
    method!("with", "with(index, value)", "Array"),
];

const TYPED_STATIC: &[Member] = &[
    prop!("BYTES_PER_ELEMENT", "BYTES_PER_ELEMENT", "Number"),
    method!("from", "from(source, mapFn)", "Array"),
    method!("of", "of(...items)", "Array"),
];

const BUFFER_INSTANCE: &[Member] = &[
    prop!("byteLength", "byteLength", "Number"),
    prop!("detached", "detached", "Boolean"),
    prop!("maxByteLength", "maxByteLength", "Number"),
    prop!("resizable", "resizable", "Boolean"),
    method!("resize", "resize(newLength)", ""),
    method!("slice", "slice(begin, end)", "ArrayBuffer"),
    method!("transfer", "transfer(newByteLength)", "ArrayBuffer"),
    method!(
        "transferToFixedLength",
        "transferToFixedLength(newByteLength)",
        "ArrayBuffer"
    ),
];

const BUFFER_STATIC: &[Member] = &[method!("isView", "ArrayBuffer.isView(value)", "Boolean")];

const DATA_VIEW: &[Member] = &[
    prop!("buffer", "buffer", "ArrayBuffer"),
    prop!("byteLength", "byteLength", "Number"),
    prop!("byteOffset", "byteOffset", "Number"),
    method!("getBigInt64", "getBigInt64(offset, littleEndian)", "BigInt"),
    method!(
        "getBigUint64",
        "getBigUint64(offset, littleEndian)",
        "BigInt"
    ),
    method!("getFloat32", "getFloat32(offset, littleEndian)", "Number"),
    method!("getFloat64", "getFloat64(offset, littleEndian)", "Number"),
    method!("getInt8", "getInt8(offset)", "Number"),
    method!("getInt16", "getInt16(offset, littleEndian)", "Number"),
    method!("getInt32", "getInt32(offset, littleEndian)", "Number"),
    method!("getUint8", "getUint8(offset)", "Number"),
    method!("getUint16", "getUint16(offset, littleEndian)", "Number"),
    method!("getUint32", "getUint32(offset, littleEndian)", "Number"),
    method!(
        "setBigInt64",
        "setBigInt64(offset, value, littleEndian)",
        ""
    ),
    method!(
        "setBigUint64",
        "setBigUint64(offset, value, littleEndian)",
        ""
    ),
    method!("setFloat32", "setFloat32(offset, value, littleEndian)", ""),
    method!("setFloat64", "setFloat64(offset, value, littleEndian)", ""),
    method!("setInt8", "setInt8(offset, value)", ""),
    method!("setInt16", "setInt16(offset, value, littleEndian)", ""),
    method!("setInt32", "setInt32(offset, value, littleEndian)", ""),
    method!("setUint8", "setUint8(offset, value)", ""),
    method!("setUint16", "setUint16(offset, value, littleEndian)", ""),
    method!("setUint32", "setUint32(offset, value, littleEndian)", ""),
];

const WEAK_REF: &[Member] = &[method!("deref", "deref()", "")];
const FINALIZATION: &[Member] = &[
    method!(
        "register",
        "register(target, heldValue, unregisterToken)",
        ""
    ),
    method!("unregister", "unregister(unregisterToken)", ""),
];

const CONSOLE_STATIC: &[Member] = &[
    method!("assert", "console.assert(condition, ...data)", ""),
    method!("clear", "console.clear()", ""),
    method!("count", "console.count(label)", ""),
    method!("countReset", "console.countReset(label)", ""),
    method!("debug", "console.debug(...data)", ""),
    method!("dir", "console.dir(item)", ""),
    method!("error", "console.error(...data)", ""),
    method!("group", "console.group(...data)", ""),
    method!("groupCollapsed", "console.groupCollapsed(...data)", ""),
    method!("groupEnd", "console.groupEnd()", ""),
    method!("info", "console.info(...data)", ""),
    method!("log", "console.log(...data)", ""),
    method!("table", "console.table(data)", ""),
    method!("time", "console.time(label)", ""),
    method!("timeEnd", "console.timeEnd(label)", ""),
    method!("timeLog", "console.timeLog(label, ...data)", ""),
    method!("trace", "console.trace(...data)", ""),
    method!("warn", "console.warn(...data)", ""),
];

const GENERATOR_INSTANCE: &[Member] = &[
    method!("drop", "drop(limit)", "Iterator"),
    method!("every", "every(callback)", "Boolean"),
    method!("filter", "filter(callback)", "Iterator"),
    method!("find", "find(callback)", ""),
    method!("flatMap", "flatMap(callback)", "Iterator"),
    method!("forEach", "forEach(callback)", ""),
    method!("map", "map(callback)", "Iterator"),
    method!("next", "next(value)", "Object"),
    method!("reduce", "reduce(callback, initial)", ""),
    method!("return", "return(value)", "Object"),
    method!("some", "some(callback)", "Boolean"),
    method!("take", "take(limit)", "Iterator"),
    method!("throw", "throw(exception)", "Object"),
    method!("toArray", "toArray()", "Array"),
];

const ITERATOR_STATIC: &[Member] = &[method!("from", "Iterator.from(object)", "Iterator")];

const EVENT_TARGET_INSTANCE: &[Member] = &[
    method!(
        "addEventListener",
        "addEventListener(type, listener, options)",
        ""
    ),
    method!(
        "removeEventListener",
        "removeEventListener(type, listener, options)",
        ""
    ),
    method!("dispatchEvent", "dispatchEvent(event)", "Boolean"),
];

const NODE_INSTANCE: &[Member] = &[
    prop!("nodeType", "nodeType", "Number"),
    prop!("nodeName", "nodeName", "String"),
    prop!("textContent", "textContent", "String"),
    prop!("parentNode", "parentNode", "Node"),
    prop!("parentElement", "parentElement", "HTMLElement"),
    prop!("childNodes", "childNodes", "NodeList"),
    prop!("firstChild", "firstChild", "Node"),
    prop!("lastChild", "lastChild", "Node"),
    prop!("nextSibling", "nextSibling", "Node"),
    prop!("previousSibling", "previousSibling", "Node"),
    method!("appendChild", "appendChild(node)", "Node"),
    method!("removeChild", "removeChild(node)", "Node"),
    method!("insertBefore", "insertBefore(node, child)", "Node"),
    method!("replaceChild", "replaceChild(node, child)", "Node"),
    method!("cloneNode", "cloneNode(deep)", "Node"),
    method!("contains", "contains(other)", "Boolean"),
    method!("hasChildNodes", "hasChildNodes()", "Boolean"),
    method!("normalize", "normalize()", ""),
    method!("isEqualNode", "isEqualNode(other)", "Boolean"),
    method!("isSameNode", "isSameNode(other)", "Boolean"),
    prop!("baseURI", "baseURI", "String"),
    prop!("isConnected", "isConnected", "Boolean"),
    prop!("nodeValue", "nodeValue", "String"),
    prop!("ownerDocument", "ownerDocument", "Document"),
    method!("getRootNode", "getRootNode(options)", "Node"),
];

const NODE_STATIC: &[Member] = &[
    prop!("ELEMENT_NODE", "Node.ELEMENT_NODE", "Number"),
    prop!("ATTRIBUTE_NODE", "Node.ATTRIBUTE_NODE", "Number"),
    prop!("TEXT_NODE", "Node.TEXT_NODE", "Number"),
    prop!("CDATA_SECTION_NODE", "Node.CDATA_SECTION_NODE", "Number"),
    prop!(
        "PROCESSING_INSTRUCTION_NODE",
        "Node.PROCESSING_INSTRUCTION_NODE",
        "Number"
    ),
    prop!("COMMENT_NODE", "Node.COMMENT_NODE", "Number"),
    prop!("DOCUMENT_NODE", "Node.DOCUMENT_NODE", "Number"),
    prop!("DOCUMENT_TYPE_NODE", "Node.DOCUMENT_TYPE_NODE", "Number"),
    prop!(
        "DOCUMENT_FRAGMENT_NODE",
        "Node.DOCUMENT_FRAGMENT_NODE",
        "Number"
    ),
];

const ELEMENT_INSTANCE: &[Member] = &[
    method!("querySelector", "querySelector(selectors)", "HTMLElement"),
    method!(
        "querySelectorAll",
        "querySelectorAll(selectors)",
        "NodeList"
    ),
    method!("closest", "closest(selectors)", "HTMLElement"),
    method!("matches", "matches(selectors)", "Boolean"),
    method!("getAttribute", "getAttribute(name)", "String"),
    method!("getAttributeNames", "getAttributeNames()", "Array"),
    method!("setAttribute", "setAttribute(name, value)", ""),
    method!("removeAttribute", "removeAttribute(name)", ""),
    method!("hasAttribute", "hasAttribute(name)", "Boolean"),
    method!("toggleAttribute", "toggleAttribute(name, force)", "Boolean"),
    prop!("classList", "classList", "DOMTokenList"),
    prop!("className", "className", "String"),
    prop!("id", "id", "String"),
    prop!("tagName", "tagName", "String"),
    prop!("innerHTML", "innerHTML", "String"),
    prop!("outerHTML", "outerHTML", "String"),
    prop!("children", "children", "HTMLCollection"),
    prop!("childElementCount", "childElementCount", "Number"),
    prop!("firstElementChild", "firstElementChild", "HTMLElement"),
    prop!("lastElementChild", "lastElementChild", "HTMLElement"),
    prop!("nextElementSibling", "nextElementSibling", "HTMLElement"),
    prop!(
        "previousElementSibling",
        "previousElementSibling",
        "HTMLElement"
    ),
    method!("append", "append(...nodes)", ""),
    method!("prepend", "prepend(...nodes)", ""),
    method!("before", "before(...nodes)", ""),
    method!("after", "after(...nodes)", ""),
    method!("replaceWith", "replaceWith(...nodes)", ""),
    method!("replaceChildren", "replaceChildren(...nodes)", ""),
    method!("remove", "remove()", ""),
    method!("scroll", "scroll(x, y)", ""),
    method!("scrollBy", "scrollBy(x, y)", ""),
    method!("scrollIntoView", "scrollIntoView(options)", ""),
    method!("scrollTo", "scrollTo(x, y)", ""),
    method!(
        "insertAdjacentElement",
        "insertAdjacentElement(position, element)",
        "HTMLElement"
    ),
    method!(
        "insertAdjacentHTML",
        "insertAdjacentHTML(position, html)",
        ""
    ),
    method!("click", "click()", ""),
    method!("focus", "focus(options)", ""),
    method!("blur", "blur()", ""),
];

const HTML_ELEMENT_INSTANCE: &[Member] = &[
    prop!("innerText", "innerText", "String"),
    prop!("hidden", "hidden", "Boolean"),
    prop!("title", "title", "String"),
    prop!("lang", "lang", "String"),
    prop!("dir", "dir", "String"),
    prop!("tabIndex", "tabIndex", "Number"),
    prop!("contentEditable", "contentEditable", "String"),
    prop!("offsetWidth", "offsetWidth", "Number"),
    prop!("offsetHeight", "offsetHeight", "Number"),
    prop!("offsetTop", "offsetTop", "Number"),
    prop!("offsetLeft", "offsetLeft", "Number"),
    prop!("clientWidth", "clientWidth", "Number"),
    prop!("clientHeight", "clientHeight", "Number"),
    prop!("scrollWidth", "scrollWidth", "Number"),
    prop!("scrollHeight", "scrollHeight", "Number"),
    prop!("scrollTop", "scrollTop", "Number"),
    prop!("scrollLeft", "scrollLeft", "Number"),
    prop!("style", "style", "CSSStyleDeclaration"),
    prop!("dataset", "dataset", "DOMStringMap"),
    prop!("accessKey", "accessKey", "String"),
    prop!("draggable", "draggable", "Boolean"),
    prop!("spellcheck", "spellcheck", "Boolean"),
    prop!("translate", "translate", "Boolean"),
    prop!("autofocus", "autofocus", "Boolean"),
    prop!("inert", "inert", "Boolean"),
    prop!("value", "value", "String"),
    prop!("checked", "checked", "Boolean"),
    prop!("disabled", "disabled", "Boolean"),
    prop!("readOnly", "readOnly", "Boolean"),
    prop!("required", "required", "Boolean"),
    prop!("multiple", "multiple", "Boolean"),
    prop!("name", "name", "String"),
    prop!("type", "type", "String"),
    prop!("placeholder", "placeholder", "String"),
    prop!("href", "href", "String"),
    prop!("target", "target", "String"),
    prop!("rel", "rel", "String"),
    prop!("src", "src", "String"),
    prop!("alt", "alt", "String"),
    prop!("width", "width", "Number"),
    prop!("height", "height", "Number"),
    prop!("action", "action", "String"),
    prop!("method", "method", "String"),
    prop!("text", "text", "String"),
    prop!("selected", "selected", "Boolean"),
    prop!("selectedIndex", "selectedIndex", "Number"),
    prop!("files", "files", "FileList"),
    method!(
        "getBoundingClientRect",
        "getBoundingClientRect()",
        "DOMRect"
    ),
    method!(
        "getContext",
        "getContext(contextId, options)",
        "CanvasRenderingContext2D"
    ),
    prop!("accept", "accept", "String"),
    prop!("autocomplete", "autocomplete", "String"),
    prop!("autoplay", "autoplay", "Boolean"),
    method!("checkValidity", "checkValidity()", "Boolean"),
    prop!("complete", "complete", "Boolean"),
    prop!("contentDocument", "contentDocument", "Document"),
    prop!("contentWindow", "contentWindow", "Window"),
    prop!("controls", "controls", "Boolean"),
    prop!("currentTime", "currentTime", "Number"),
    prop!("defaultChecked", "defaultChecked", "Boolean"),
    prop!("defaultValue", "defaultValue", "String"),
    prop!("download", "download", "String"),
    prop!("duration", "duration", "Number"),
    prop!("elements", "elements", "HTMLCollection"),
    prop!("ended", "ended", "Boolean"),
    prop!("form", "form", "HTMLElement"),
    prop!("htmlFor", "htmlFor", "String"),
    prop!("indeterminate", "indeterminate", "Boolean"),
    method!("load", "load()", ""),
    prop!("loop", "loop", "Boolean"),
    prop!("max", "max", "String"),
    prop!("maxLength", "maxLength", "Number"),
    prop!("min", "min", "String"),
    prop!("minLength", "minLength", "Number"),
    prop!("muted", "muted", "Boolean"),
    prop!("naturalHeight", "naturalHeight", "Number"),
    prop!("naturalWidth", "naturalWidth", "Number"),
    prop!("open", "open", "Boolean"),
    prop!("options", "options", "HTMLCollection"),
    prop!("pattern", "pattern", "String"),
    prop!("paused", "paused", "Boolean"),
    method!("pause", "pause()", ""),
    prop!("playbackRate", "playbackRate", "Number"),
    method!("play", "play()", "Promise"),
    method!("reportValidity", "reportValidity()", "Boolean"),
    method!("reset", "reset()", ""),
    prop!("rows", "rows", "Number"),
    prop!("cols", "cols", "Number"),
    method!("select", "select()", ""),
    prop!("selectedOptions", "selectedOptions", "HTMLCollection"),
    method!("setCustomValidity", "setCustomValidity(message)", ""),
    prop!("step", "step", "String"),
    method!("submit", "submit()", ""),
    prop!("valueAsNumber", "valueAsNumber", "Number"),
    prop!("volume", "volume", "Number"),
];

const DOCUMENT_INSTANCE: &[Member] = &[
    method!("querySelector", "querySelector(selectors)", "HTMLElement"),
    method!(
        "querySelectorAll",
        "querySelectorAll(selectors)",
        "NodeList"
    ),
    method!("getElementById", "getElementById(id)", "HTMLElement"),
    method!(
        "getElementsByClassName",
        "getElementsByClassName(names)",
        "HTMLCollection"
    ),
    method!(
        "getElementsByTagName",
        "getElementsByTagName(name)",
        "HTMLCollection"
    ),
    method!("getElementsByName", "getElementsByName(name)", "NodeList"),
    method!(
        "createElement",
        "createElement(tagName, options)",
        "HTMLElement"
    ),
    method!(
        "createElementNS",
        "createElementNS(namespace, tagName)",
        "HTMLElement"
    ),
    method!("createTextNode", "createTextNode(data)", "Text"),
    prop!("body", "body", "HTMLElement"),
    prop!("head", "head", "HTMLElement"),
    prop!("documentElement", "documentElement", "HTMLElement"),
    prop!("title", "title", "String"),
    prop!("cookie", "cookie", "String"),
    prop!("URL", "URL", "String"),
    prop!("documentURI", "documentURI", "String"),
    prop!("forms", "forms", "HTMLCollection"),
    prop!("images", "images", "HTMLCollection"),
    prop!("links", "links", "HTMLCollection"),
    prop!("scripts", "scripts", "HTMLCollection"),
    prop!("readyState", "readyState", "String"),
    prop!("referrer", "referrer", "String"),
    prop!("characterSet", "characterSet", "String"),
    prop!("contentType", "contentType", "String"),
    prop!("activeElement", "activeElement", "HTMLElement"),
    prop!("defaultView", "defaultView", "Window"),
    method!("hasFocus", "hasFocus()", "Boolean"),
    method!("adoptNode", "adoptNode(node)", "Node"),
    method!("importNode", "importNode(node, deep)", "Node"),
    method!("elementFromPoint", "elementFromPoint(x, y)", "HTMLElement"),
    method!("elementsFromPoint", "elementsFromPoint(x, y)", "Array"),
    method!("write", "write(...text)", ""),
    method!("writeln", "writeln(...text)", ""),
    prop!("hidden", "hidden", "Boolean"),
    prop!("visibilityState", "visibilityState", "String"),
    prop!("scrollingElement", "scrollingElement", "HTMLElement"),
    prop!("currentScript", "currentScript", "HTMLElement"),
    method!(
        "createDocumentFragment",
        "createDocumentFragment()",
        "DocumentFragment"
    ),
    method!("createComment", "createComment(data)", "Comment"),
    method!("createRange", "createRange()", "Range"),
    method!("getSelection", "getSelection()", "Selection"),
    prop!("fullscreenElement", "fullscreenElement", "HTMLElement"),
    method!("exitFullscreen", "exitFullscreen()", "Promise"),
];

const NODE_LIST_INSTANCE: &[Member] = &[
    prop!("length", "length", "Number"),
    method!("item", "item(index)", "HTMLElement"),
    method!("entries", "entries()", "Array"),
    method!("keys", "keys()", "Array"),
    method!("values", "values()", "Array"),
    method!("forEach", "forEach(callback, thisArg)", ""),
];

const HTML_COLLECTION_INSTANCE: &[Member] = &[
    prop!("length", "length", "Number"),
    method!("item", "item(index)", "HTMLElement"),
    method!("namedItem", "namedItem(name)", "HTMLElement"),
    method!("forEach", "forEach(callback, thisArg)", ""),
];

const DOM_TOKEN_LIST_INSTANCE: &[Member] = &[
    prop!("length", "length", "Number"),
    prop!("value", "value", "String"),
    method!("item", "item(index)", "String"),
    method!("contains", "contains(token)", "Boolean"),
    method!("add", "add(...tokens)", ""),
    method!("remove", "remove(...tokens)", ""),
    method!("toggle", "toggle(token, force)", "Boolean"),
    method!("replace", "replace(oldToken, newToken)", "Boolean"),
    method!("supports", "supports(token)", "Boolean"),
    method!("forEach", "forEach(callback, thisArg)", ""),
];

const TEXT_INSTANCE: &[Member] = &[
    prop!("data", "data", "String"),
    prop!("length", "length", "Number"),
    prop!("wholeText", "wholeText", "String"),
    method!("splitText", "splitText(offset)", "Text"),
];

const WINDOW_INSTANCE: &[Member] = &[
    prop!("document", "document", "Document"),
    prop!("location", "location", "Location"),
    prop!("navigator", "navigator", "Navigator"),
    prop!("localStorage", "localStorage", "Storage"),
    prop!("sessionStorage", "sessionStorage", "Storage"),
    prop!("innerWidth", "innerWidth", "Number"),
    prop!("innerHeight", "innerHeight", "Number"),
    prop!("outerWidth", "outerWidth", "Number"),
    prop!("outerHeight", "outerHeight", "Number"),
    prop!("scrollX", "scrollX", "Number"),
    prop!("scrollY", "scrollY", "Number"),
    prop!("pageXOffset", "pageXOffset", "Number"),
    prop!("pageYOffset", "pageYOffset", "Number"),
    prop!("devicePixelRatio", "devicePixelRatio", "Number"),
    prop!("name", "name", "String"),
    prop!("origin", "origin", "String"),
    prop!("closed", "closed", "Boolean"),
    prop!("parent", "parent", "Window"),
    prop!("top", "top", "Window"),
    prop!("self", "self", "Window"),
    prop!("frames", "frames", "Window"),
    prop!("opener", "opener", "Window"),
    prop!("frameElement", "frameElement", "HTMLElement"),
    prop!("history", "history", "History"),
    prop!("crypto", "crypto", "Crypto"),
    prop!("performance", "performance", "Performance"),
    prop!("customElements", "customElements", "CustomElementRegistry"),
    prop!("indexedDB", "indexedDB", "IDBFactory"),
    prop!("screen", "screen", "Screen"),
    prop!("visualViewport", "visualViewport", "VisualViewport"),
    prop!("isSecureContext", "isSecureContext", "Boolean"),
    method!("alert", "alert(message)", ""),
    method!("confirm", "confirm(message)", "Boolean"),
    method!("prompt", "prompt(message, defaultValue)", "String"),
    method!("open", "open(url, target, features)", "Window"),
    method!("close", "close()", ""),
    method!("focus", "focus()", ""),
    method!("blur", "blur()", ""),
    method!("stop", "stop()", ""),
    method!("scroll", "scroll(x, y)", ""),
    method!("scrollBy", "scrollBy(x, y)", ""),
    method!("scrollTo", "scrollTo(x, y)", ""),
    method!(
        "getComputedStyle",
        "getComputedStyle(element, pseudoElt)",
        "CSSStyleDeclaration"
    ),
    method!(
        "requestAnimationFrame",
        "requestAnimationFrame(callback)",
        "Number"
    ),
    method!("cancelAnimationFrame", "cancelAnimationFrame(handle)", ""),
    method!(
        "setTimeout",
        "setTimeout(handler, timeout, ...arguments)",
        "Number"
    ),
    method!("clearTimeout", "clearTimeout(handle)", ""),
    method!(
        "setInterval",
        "setInterval(handler, timeout, ...arguments)",
        "Number"
    ),
    method!("clearInterval", "clearInterval(handle)", ""),
    method!("fetch", "fetch(input, init)", "Promise"),
    method!("atob", "atob(data)", "String"),
    method!("btoa", "btoa(data)", "String"),
    method!(
        "postMessage",
        "postMessage(message, targetOrigin, transfer)",
        ""
    ),
    method!("getSelection", "getSelection()", "Selection"),
    method!("matchMedia", "matchMedia(query)", "MediaQueryList"),
    method!(
        "requestIdleCallback",
        "requestIdleCallback(callback, options)",
        "Number"
    ),
    method!("cancelIdleCallback", "cancelIdleCallback(handle)", ""),
];

const LOCATION_INSTANCE: &[Member] = &[
    prop!("href", "href", "String"),
    prop!("origin", "origin", "String"),
    prop!("protocol", "protocol", "String"),
    prop!("host", "host", "String"),
    prop!("hostname", "hostname", "String"),
    prop!("port", "port", "String"),
    prop!("pathname", "pathname", "String"),
    prop!("search", "search", "String"),
    prop!("hash", "hash", "String"),
    method!("assign", "assign(url)", ""),
    method!("replace", "replace(url)", ""),
    method!("reload", "reload()", ""),
    method!("toString", "toString()", "String"),
];

const NAVIGATOR_INSTANCE: &[Member] = &[
    prop!("userAgent", "userAgent", "String"),
    prop!("language", "language", "String"),
    prop!("languages", "languages", "Array"),
    prop!("platform", "platform", "String"),
    prop!("cookieEnabled", "cookieEnabled", "Boolean"),
    prop!("onLine", "onLine", "Boolean"),
    prop!("hardwareConcurrency", "hardwareConcurrency", "Number"),
    prop!("maxTouchPoints", "maxTouchPoints", "Number"),
    prop!("vendor", "vendor", "String"),
    method!("sendBeacon", "sendBeacon(url, data)", "Boolean"),
    prop!("clipboard", "clipboard", "Clipboard"),
];

const STORAGE_INSTANCE: &[Member] = &[
    prop!("length", "length", "Number"),
    method!("key", "key(index)", "String"),
    method!("getItem", "getItem(key)", "String"),
    method!("setItem", "setItem(key, value)", ""),
    method!("removeItem", "removeItem(key)", ""),
    method!("clear", "clear()", ""),
];

const CSS_STYLE_INSTANCE: &[Member] = &[
    prop!("cssText", "cssText", "String"),
    prop!("length", "length", "Number"),
    method!("getPropertyValue", "getPropertyValue(property)", "String"),
    method!("setProperty", "setProperty(property, value, priority)", ""),
    method!("removeProperty", "removeProperty(property)", "String"),
    method!("item", "item(index)", "String"),
    prop!("alignContent", "alignContent", "String"),
    prop!("alignItems", "alignItems", "String"),
    prop!("animation", "animation", "String"),
    prop!("background", "background", "String"),
    prop!("backgroundColor", "backgroundColor", "String"),
    prop!("backgroundImage", "backgroundImage", "String"),
    prop!("border", "border", "String"),
    prop!("borderColor", "borderColor", "String"),
    prop!("borderRadius", "borderRadius", "String"),
    prop!("borderWidth", "borderWidth", "String"),
    prop!("bottom", "bottom", "String"),
    prop!("boxSizing", "boxSizing", "String"),
    prop!("color", "color", "String"),
    prop!("columnGap", "columnGap", "String"),
    prop!("cursor", "cursor", "String"),
    prop!("display", "display", "String"),
    prop!("flex", "flex", "String"),
    prop!("flexDirection", "flexDirection", "String"),
    prop!("flexWrap", "flexWrap", "String"),
    prop!("font", "font", "String"),
    prop!("fontFamily", "fontFamily", "String"),
    prop!("fontSize", "fontSize", "String"),
    prop!("fontWeight", "fontWeight", "String"),
    prop!("gap", "gap", "String"),
    prop!("gridTemplateColumns", "gridTemplateColumns", "String"),
    prop!("gridTemplateRows", "gridTemplateRows", "String"),
    prop!("height", "height", "String"),
    prop!("justifyContent", "justifyContent", "String"),
    prop!("left", "left", "String"),
    prop!("lineHeight", "lineHeight", "String"),
    prop!("margin", "margin", "String"),
    prop!("marginBottom", "marginBottom", "String"),
    prop!("marginLeft", "marginLeft", "String"),
    prop!("marginRight", "marginRight", "String"),
    prop!("marginTop", "marginTop", "String"),
    prop!("maxHeight", "maxHeight", "String"),
    prop!("maxWidth", "maxWidth", "String"),
    prop!("minHeight", "minHeight", "String"),
    prop!("minWidth", "minWidth", "String"),
    prop!("objectFit", "objectFit", "String"),
    prop!("opacity", "opacity", "String"),
    prop!("overflow", "overflow", "String"),
    prop!("overflowX", "overflowX", "String"),
    prop!("overflowY", "overflowY", "String"),
    prop!("padding", "padding", "String"),
    prop!("paddingBottom", "paddingBottom", "String"),
    prop!("paddingLeft", "paddingLeft", "String"),
    prop!("paddingRight", "paddingRight", "String"),
    prop!("paddingTop", "paddingTop", "String"),
    prop!("pointerEvents", "pointerEvents", "String"),
    prop!("position", "position", "String"),
    prop!("right", "right", "String"),
    prop!("rowGap", "rowGap", "String"),
    prop!("textAlign", "textAlign", "String"),
    prop!("textDecoration", "textDecoration", "String"),
    prop!("top", "top", "String"),
    prop!("transform", "transform", "String"),
    prop!("transition", "transition", "String"),
    prop!("visibility", "visibility", "String"),
    prop!("whiteSpace", "whiteSpace", "String"),
    prop!("width", "width", "String"),
    prop!("zIndex", "zIndex", "String"),
];

const EVENT_INSTANCE: &[Member] = &[
    prop!("type", "type", "String"),
    prop!("target", "target", "EventTarget"),
    prop!("currentTarget", "currentTarget", "EventTarget"),
    prop!("bubbles", "bubbles", "Boolean"),
    prop!("cancelable", "cancelable", "Boolean"),
    prop!("defaultPrevented", "defaultPrevented", "Boolean"),
    prop!("eventPhase", "eventPhase", "Number"),
    prop!("timeStamp", "timeStamp", "Number"),
    method!("preventDefault", "preventDefault()", ""),
    method!("stopPropagation", "stopPropagation()", ""),
    method!("stopImmediatePropagation", "stopImmediatePropagation()", ""),
];

const URL_INSTANCE: &[Member] = &[
    prop!("href", "href", "String"),
    prop!("origin", "origin", "String"),
    prop!("protocol", "protocol", "String"),
    prop!("username", "username", "String"),
    prop!("password", "password", "String"),
    prop!("host", "host", "String"),
    prop!("hostname", "hostname", "String"),
    prop!("port", "port", "String"),
    prop!("pathname", "pathname", "String"),
    prop!("search", "search", "String"),
    prop!("searchParams", "searchParams", "URLSearchParams"),
    prop!("hash", "hash", "String"),
    method!("toJSON", "toJSON()", "String"),
    method!("toString", "toString()", "String"),
];
const URL_STATIC: &[Member] = &[
    method!("canParse", "URL.canParse(url, base)", "Boolean"),
    method!("parse", "URL.parse(url, base)", "URL"),
];
const URL_SEARCH_PARAMS: &[Member] = &[
    prop!("size", "size", "Number"),
    method!("append", "append(name, value)", ""),
    method!("delete", "delete(name, value)", ""),
    method!("get", "get(name)", "String"),
    method!("getAll", "getAll(name)", "Array"),
    method!("has", "has(name, value)", "Boolean"),
    method!("set", "set(name, value)", ""),
    method!("sort", "sort()", ""),
    method!("entries", "entries()", "Iterator"),
    method!("keys", "keys()", "Iterator"),
    method!("values", "values()", "Iterator"),
    method!("forEach", "forEach(callback)", ""),
    method!("toString", "toString()", "String"),
];
const FORM_DATA: &[Member] = &[
    method!("append", "append(name, value, filename)", ""),
    method!("delete", "delete(name)", ""),
    method!("get", "get(name)", ""),
    method!("getAll", "getAll(name)", "Array"),
    method!("has", "has(name)", "Boolean"),
    method!("set", "set(name, value, filename)", ""),
    method!("entries", "entries()", "Iterator"),
    method!("keys", "keys()", "Iterator"),
    method!("values", "values()", "Iterator"),
    method!("forEach", "forEach(callback)", ""),
];
const HEADERS_INSTANCE: &[Member] = &[
    method!("append", "append(name, value)", ""),
    method!("delete", "delete(name)", ""),
    method!("get", "get(name)", "String"),
    method!("has", "has(name)", "Boolean"),
    method!("set", "set(name, value)", ""),
    method!("entries", "entries()", "Iterator"),
    method!("keys", "keys()", "Iterator"),
    method!("values", "values()", "Iterator"),
    method!("forEach", "forEach(callback)", ""),
];
const REQUEST_INSTANCE: &[Member] = &[
    prop!("bodyUsed", "bodyUsed", "Boolean"),
    prop!("cache", "cache", "String"),
    prop!("credentials", "credentials", "String"),
    prop!("headers", "headers", "Headers"),
    prop!("method", "method", "String"),
    prop!("mode", "mode", "String"),
    prop!("redirect", "redirect", "String"),
    prop!("signal", "signal", "AbortSignal"),
    prop!("url", "url", "String"),
    prop!("body", "body", "ReadableStream"),
    method!("arrayBuffer", "arrayBuffer()", "Promise"),
    method!("blob", "blob()", "Promise"),
    method!("clone", "clone()", "Request"),
    method!("formData", "formData()", "Promise"),
    method!("json", "json()", "Promise"),
    method!("text", "text()", "Promise"),
];
const RESPONSE_INSTANCE: &[Member] = &[
    prop!("bodyUsed", "bodyUsed", "Boolean"),
    prop!("headers", "headers", "Headers"),
    prop!("ok", "ok", "Boolean"),
    prop!("redirected", "redirected", "Boolean"),
    prop!("status", "status", "Number"),
    prop!("statusText", "statusText", "String"),
    prop!("type", "type", "String"),
    prop!("url", "url", "String"),
    prop!("body", "body", "ReadableStream"),
    method!("arrayBuffer", "arrayBuffer()", "Promise"),
    method!("blob", "blob()", "Promise"),
    method!("clone", "clone()", "Response"),
    method!("formData", "formData()", "Promise"),
    method!("json", "json()", "Promise"),
    method!("text", "text()", "Promise"),
];
const RESPONSE_STATIC: &[Member] = &[
    method!("error", "Response.error()", "Response"),
    method!("json", "Response.json(data, options)", "Response"),
    method!("redirect", "Response.redirect(url, status)", "Response"),
];
const ABORT_CONTROLLER: &[Member] = &[
    prop!("signal", "signal", "AbortSignal"),
    method!("abort", "abort(reason)", ""),
];
const ABORT_SIGNAL: &[Member] = &[
    prop!("aborted", "aborted", "Boolean"),
    prop!("reason", "reason", ""),
    method!("throwIfAborted", "throwIfAborted()", ""),
];
const ABORT_SIGNAL_STATIC: &[Member] = &[
    method!("abort", "AbortSignal.abort(reason)", "AbortSignal"),
    method!("any", "AbortSignal.any(signals)", "AbortSignal"),
    method!(
        "timeout",
        "AbortSignal.timeout(milliseconds)",
        "AbortSignal"
    ),
];
const BLOB_INSTANCE: &[Member] = &[
    prop!("size", "size", "Number"),
    prop!("type", "type", "String"),
    method!("arrayBuffer", "arrayBuffer()", "Promise"),
    method!("bytes", "bytes()", "Promise"),
    method!("slice", "slice(start, end, type)", "Blob"),
    method!("text", "text()", "Promise"),
];
const FILE_INSTANCE: &[Member] = &[
    prop!("name", "name", "String"),
    prop!("lastModified", "lastModified", "Number"),
];
const FILE_READER: &[Member] = &[
    prop!("error", "error", "Error"),
    prop!("readyState", "readyState", "Number"),
    prop!("result", "result", ""),
    method!("abort", "abort()", ""),
    method!("readAsArrayBuffer", "readAsArrayBuffer(blob)", ""),
    method!("readAsDataURL", "readAsDataURL(blob)", ""),
    method!("readAsText", "readAsText(blob, encoding)", ""),
];
const FILE_LIST: &[Member] = &[
    prop!("length", "length", "Number"),
    method!("item", "item(index)", "File"),
];
const TEXT_ENCODER: &[Member] = &[
    prop!("encoding", "encoding", "String"),
    method!("encode", "encode(input)", "Uint8Array"),
    method!("encodeInto", "encodeInto(input, destination)", "Object"),
];
const TEXT_DECODER: &[Member] = &[
    prop!("encoding", "encoding", "String"),
    method!("decode", "decode(input, options)", "String"),
];
const DOM_PARSER: &[Member] = &[method!(
    "parseFromString",
    "parseFromString(text, type)",
    "Document"
)];
const XHR_INSTANCE: &[Member] = &[
    prop!("readyState", "readyState", "Number"),
    prop!("response", "response", ""),
    prop!("responseText", "responseText", "String"),
    prop!("responseType", "responseType", "String"),
    prop!("responseURL", "responseURL", "String"),
    prop!("status", "status", "Number"),
    prop!("statusText", "statusText", "String"),
    method!("abort", "abort()", ""),
    method!("getAllResponseHeaders", "getAllResponseHeaders()", "String"),
    method!("getResponseHeader", "getResponseHeader(name)", "String"),
    method!("open", "open(method, url, async, user, password)", ""),
    method!("overrideMimeType", "overrideMimeType(mime)", ""),
    method!("send", "send(body)", ""),
    method!("setRequestHeader", "setRequestHeader(name, value)", ""),
];
const WEB_SOCKET: &[Member] = &[
    prop!("binaryType", "binaryType", "String"),
    prop!("bufferedAmount", "bufferedAmount", "Number"),
    prop!("protocol", "protocol", "String"),
    prop!("readyState", "readyState", "Number"),
    prop!("url", "url", "String"),
    method!("close", "close(code, reason)", ""),
    method!("send", "send(data)", ""),
];
const MUTATION_OBSERVER: &[Member] = &[
    method!("disconnect", "disconnect()", ""),
    method!("observe", "observe(target, options)", ""),
    method!("takeRecords", "takeRecords()", "Array"),
];
const INTERSECTION_OBSERVER: &[Member] = &[
    prop!("root", "root", "HTMLElement"),
    prop!("rootMargin", "rootMargin", "String"),
    prop!("thresholds", "thresholds", "Array"),
    method!("disconnect", "disconnect()", ""),
    method!("observe", "observe(target)", ""),
    method!("takeRecords", "takeRecords()", "Array"),
    method!("unobserve", "unobserve(target)", ""),
];
const RESIZE_OBSERVER: &[Member] = &[
    method!("disconnect", "disconnect()", ""),
    method!("observe", "observe(target, options)", ""),
    method!("unobserve", "unobserve(target)", ""),
];
const CUSTOM_EVENT: &[Member] = &[prop!("detail", "detail", "")];
const MOUSE_EVENT: &[Member] = &[
    prop!("altKey", "altKey", "Boolean"),
    prop!("button", "button", "Number"),
    prop!("buttons", "buttons", "Number"),
    prop!("clientX", "clientX", "Number"),
    prop!("clientY", "clientY", "Number"),
    prop!("ctrlKey", "ctrlKey", "Boolean"),
    prop!("metaKey", "metaKey", "Boolean"),
    prop!("movementX", "movementX", "Number"),
    prop!("movementY", "movementY", "Number"),
    prop!("pageX", "pageX", "Number"),
    prop!("pageY", "pageY", "Number"),
    prop!("screenX", "screenX", "Number"),
    prop!("screenY", "screenY", "Number"),
    prop!("shiftKey", "shiftKey", "Boolean"),
];
const KEYBOARD_EVENT: &[Member] = &[
    prop!("altKey", "altKey", "Boolean"),
    prop!("code", "code", "String"),
    prop!("ctrlKey", "ctrlKey", "Boolean"),
    prop!("key", "key", "String"),
    prop!("location", "location", "Number"),
    prop!("metaKey", "metaKey", "Boolean"),
    prop!("repeat", "repeat", "Boolean"),
    prop!("shiftKey", "shiftKey", "Boolean"),
];
const INPUT_EVENT: &[Member] = &[
    prop!("data", "data", "String"),
    prop!("inputType", "inputType", "String"),
    prop!("isComposing", "isComposing", "Boolean"),
];
const FOCUS_EVENT: &[Member] = &[prop!("relatedTarget", "relatedTarget", "EventTarget")];
const HISTORY_INSTANCE: &[Member] = &[
    prop!("length", "length", "Number"),
    prop!("scrollRestoration", "scrollRestoration", "String"),
    prop!("state", "state", ""),
    method!("back", "back()", ""),
    method!("forward", "forward()", ""),
    method!("go", "go(delta)", ""),
    method!("pushState", "pushState(data, unused, url)", ""),
    method!("replaceState", "replaceState(data, unused, url)", ""),
];
const CRYPTO_INSTANCE: &[Member] = &[
    prop!("subtle", "subtle", "SubtleCrypto"),
    method!("getRandomValues", "getRandomValues(array)", ""),
    method!("randomUUID", "randomUUID()", "String"),
];
const SUBTLE_CRYPTO: &[Member] = &[
    method!("decrypt", "decrypt(algorithm, key, data)", "Promise"),
    method!(
        "deriveBits",
        "deriveBits(algorithm, key, length)",
        "Promise"
    ),
    method!(
        "deriveKey",
        "deriveKey(algorithm, key, derived, extractable, usages)",
        "Promise"
    ),
    method!("digest", "digest(algorithm, data)", "Promise"),
    method!("encrypt", "encrypt(algorithm, key, data)", "Promise"),
    method!("exportKey", "exportKey(format, key)", "Promise"),
    method!(
        "generateKey",
        "generateKey(algorithm, extractable, usages)",
        "Promise"
    ),
    method!(
        "importKey",
        "importKey(format, keyData, algorithm, extractable, usages)",
        "Promise"
    ),
    method!("sign", "sign(algorithm, key, data)", "Promise"),
    method!(
        "verify",
        "verify(algorithm, key, signature, data)",
        "Promise"
    ),
];
const PERFORMANCE_INSTANCE: &[Member] = &[
    prop!("timeOrigin", "timeOrigin", "Number"),
    method!("clearMarks", "clearMarks(name)", ""),
    method!("clearMeasures", "clearMeasures(name)", ""),
    method!("getEntries", "getEntries()", "Array"),
    method!("getEntriesByName", "getEntriesByName(name, type)", "Array"),
    method!("getEntriesByType", "getEntriesByType(type)", "Array"),
    method!("mark", "mark(name, options)", "Object"),
    method!("measure", "measure(name, start, end)", "Object"),
    method!("now", "now()", "Number"),
];
const MEDIA_QUERY_LIST: &[Member] = &[
    prop!("matches", "matches", "Boolean"),
    prop!("media", "media", "String"),
    method!("addListener", "addListener(callback)", ""),
    method!("removeListener", "removeListener(callback)", ""),
];
const DOM_RECT: &[Member] = &[
    prop!("x", "x", "Number"),
    prop!("y", "y", "Number"),
    prop!("width", "width", "Number"),
    prop!("height", "height", "Number"),
    prop!("top", "top", "Number"),
    prop!("right", "right", "Number"),
    prop!("bottom", "bottom", "Number"),
    prop!("left", "left", "Number"),
    method!("toJSON", "toJSON()", "Object"),
];
const DOCUMENT_FRAGMENT: &[Member] = &[
    prop!("childElementCount", "childElementCount", "Number"),
    prop!("children", "children", "HTMLCollection"),
    prop!("firstElementChild", "firstElementChild", "HTMLElement"),
    prop!("lastElementChild", "lastElementChild", "HTMLElement"),
    method!("append", "append(...nodes)", ""),
    method!("getElementById", "getElementById(id)", "HTMLElement"),
    method!("prepend", "prepend(...nodes)", ""),
    method!("querySelector", "querySelector(selectors)", "HTMLElement"),
    method!(
        "querySelectorAll",
        "querySelectorAll(selectors)",
        "NodeList"
    ),
    method!("replaceChildren", "replaceChildren(...nodes)", ""),
];
const SELECTION_INSTANCE: &[Member] = &[
    prop!("anchorNode", "anchorNode", "Node"),
    prop!("focusNode", "focusNode", "Node"),
    prop!("isCollapsed", "isCollapsed", "Boolean"),
    prop!("rangeCount", "rangeCount", "Number"),
    prop!("type", "type", "String"),
    method!("addRange", "addRange(range)", ""),
    method!("collapse", "collapse(node, offset)", ""),
    method!("collapseToEnd", "collapseToEnd()", ""),
    method!("collapseToStart", "collapseToStart()", ""),
    method!("containsNode", "containsNode(node, partial)", "Boolean"),
    method!("deleteFromDocument", "deleteFromDocument()", ""),
    method!("getRangeAt", "getRangeAt(index)", "Range"),
    method!("removeAllRanges", "removeAllRanges()", ""),
    method!("selectAllChildren", "selectAllChildren(node)", ""),
    method!("toString", "toString()", "String"),
];
const CUSTOM_ELEMENTS: &[Member] = &[
    method!("define", "define(name, constructor, options)", ""),
    method!("get", "get(name)", "Function"),
    method!("getName", "getName(constructor)", "String"),
    method!("upgrade", "upgrade(root)", ""),
    method!("whenDefined", "whenDefined(name)", "Promise"),
];
const CLIPBOARD_INSTANCE: &[Member] = &[
    method!("read", "read()", "Promise"),
    method!("readText", "readText()", "Promise"),
    method!("write", "write(data)", "Promise"),
    method!("writeText", "writeText(text)", "Promise"),
];
const WORKER_INSTANCE: &[Member] = &[
    method!("postMessage", "postMessage(message, transfer)", ""),
    method!("terminate", "terminate()", ""),
];
const BROADCAST_CHANNEL: &[Member] = &[
    prop!("name", "name", "String"),
    method!("close", "close()", ""),
    method!("postMessage", "postMessage(message)", ""),
];
const CANVAS_CONTEXT: &[Member] = &[
    prop!("canvas", "canvas", "HTMLElement"),
    prop!("fillStyle", "fillStyle", "String"),
    prop!("font", "font", "String"),
    prop!("globalAlpha", "globalAlpha", "Number"),
    prop!("lineWidth", "lineWidth", "Number"),
    prop!("strokeStyle", "strokeStyle", "String"),
    method!(
        "arc",
        "arc(x, y, radius, startAngle, endAngle, counterclockwise)",
        ""
    ),
    method!("beginPath", "beginPath()", ""),
    method!("clearRect", "clearRect(x, y, width, height)", ""),
    method!("drawImage", "drawImage(image, dx, dy)", ""),
    method!("fill", "fill()", ""),
    method!("fillRect", "fillRect(x, y, width, height)", ""),
    method!("fillText", "fillText(text, x, y, maxWidth)", ""),
    method!(
        "getImageData",
        "getImageData(x, y, width, height)",
        "Object"
    ),
    method!("lineTo", "lineTo(x, y)", ""),
    method!("measureText", "measureText(text)", "TextMetrics"),
    method!("moveTo", "moveTo(x, y)", ""),
    method!("putImageData", "putImageData(data, dx, dy)", ""),
    method!("rect", "rect(x, y, width, height)", ""),
    method!("restore", "restore()", ""),
    method!("rotate", "rotate(angle)", ""),
    method!("save", "save()", ""),
    method!("scale", "scale(x, y)", ""),
    method!("stroke", "stroke()", ""),
    method!("strokeRect", "strokeRect(x, y, width, height)", ""),
    method!("strokeText", "strokeText(text, x, y, maxWidth)", ""),
    method!("translate", "translate(x, y)", ""),
];
const TEXT_METRICS: &[Member] = &[prop!("width", "width", "Number")];
const IDB_FACTORY: &[Member] = &[
    method!("databases", "databases()", "Promise"),
    method!("deleteDatabase", "deleteDatabase(name)", "IDBRequest"),
    method!("open", "open(name, version)", "IDBRequest"),
];
const IDB_DATABASE: &[Member] = &[
    prop!("name", "name", "String"),
    prop!("objectStoreNames", "objectStoreNames", "Object"),
    prop!("version", "version", "Number"),
    method!("close", "close()", ""),
    method!(
        "createObjectStore",
        "createObjectStore(name, options)",
        "IDBObjectStore"
    ),
    method!("deleteObjectStore", "deleteObjectStore(name)", ""),
    method!(
        "transaction",
        "transaction(storeNames, mode, options)",
        "IDBTransaction"
    ),
];
const IDB_TRANSACTION: &[Member] = &[
    prop!("db", "db", "IDBDatabase"),
    prop!("mode", "mode", "String"),
    method!("abort", "abort()", ""),
    method!("commit", "commit()", ""),
    method!("objectStore", "objectStore(name)", "IDBObjectStore"),
];
const IDB_OBJECT_STORE: &[Member] = &[
    prop!("name", "name", "String"),
    prop!("keyPath", "keyPath", ""),
    prop!("autoIncrement", "autoIncrement", "Boolean"),
    method!("add", "add(value, key)", "IDBRequest"),
    method!("clear", "clear()", "IDBRequest"),
    method!("count", "count(query)", "IDBRequest"),
    method!(
        "createIndex",
        "createIndex(name, keyPath, options)",
        "IDBIndex"
    ),
    method!("delete", "delete(query)", "IDBRequest"),
    method!("get", "get(query)", "IDBRequest"),
    method!("getAll", "getAll(query, count)", "IDBRequest"),
    method!("index", "index(name)", "IDBIndex"),
    method!("openCursor", "openCursor(query, direction)", "IDBRequest"),
    method!("put", "put(value, key)", "IDBRequest"),
];
const IDB_INDEX: &[Member] = &[
    prop!("name", "name", "String"),
    prop!("unique", "unique", "Boolean"),
    method!("get", "get(query)", "IDBRequest"),
    method!("getAll", "getAll(query, count)", "IDBRequest"),
    method!("openCursor", "openCursor(query, direction)", "IDBRequest"),
];
const IDB_REQUEST: &[Member] = &[
    prop!("error", "error", "Error"),
    prop!("readyState", "readyState", "String"),
    prop!("result", "result", ""),
];
const IDB_CURSOR: &[Member] = &[
    prop!("key", "key", ""),
    prop!("primaryKey", "primaryKey", ""),
    prop!("value", "value", ""),
    method!("continue", "continue(key)", ""),
    method!("delete", "delete()", "IDBRequest"),
    method!("update", "update(value)", "IDBRequest"),
];
const INTL_FORMAT: &[Member] = &[
    method!("format", "format(value)", "String"),
    method!("formatToParts", "formatToParts(value)", "Array"),
    method!("resolvedOptions", "resolvedOptions()", "Object"),
];
const COLLATOR_INSTANCE: &[Member] = &[
    method!("compare", "compare(a, b)", "Number"),
    method!("resolvedOptions", "resolvedOptions()", "Object"),
];
const PLURAL_RULES: &[Member] = &[
    method!("resolvedOptions", "resolvedOptions()", "Object"),
    method!("select", "select(value)", "String"),
];
const LOCALE_INSTANCE: &[Member] = &[
    prop!("baseName", "baseName", "String"),
    prop!("language", "language", "String"),
    method!("maximize", "maximize()", "Locale"),
    method!("minimize", "minimize()", "Locale"),
    method!("toString", "toString()", "String"),
];
const SEGMENTER_INSTANCE: &[Member] = &[
    method!("resolvedOptions", "resolvedOptions()", "Object"),
    method!("segment", "segment(input)", "Object"),
];
const DISPLAY_NAMES: &[Member] = &[
    method!("of", "of(code)", "String"),
    method!("resolvedOptions", "resolvedOptions()", "Object"),
];

const COMMENT_INSTANCE: &[Member] = &[
    prop!("data", "data", "String"),
    prop!("length", "length", "Number"),
];

const RANGE_INSTANCE: &[Member] = &[
    prop!("collapsed", "collapsed", "Boolean"),
    prop!("commonAncestorContainer", "commonAncestorContainer", "Node"),
    prop!("endContainer", "endContainer", "Node"),
    prop!("endOffset", "endOffset", "Number"),
    prop!("startContainer", "startContainer", "Node"),
    prop!("startOffset", "startOffset", "Number"),
    method!("cloneContents", "cloneContents()", "DocumentFragment"),
    method!("cloneRange", "cloneRange()", "Range"),
    method!("collapse", "collapse(toStart)", ""),
    method!("deleteContents", "deleteContents()", ""),
    method!("extractContents", "extractContents()", "DocumentFragment"),
    method!(
        "getBoundingClientRect",
        "getBoundingClientRect()",
        "DOMRect"
    ),
    method!("insertNode", "insertNode(node)", ""),
    method!("selectNode", "selectNode(node)", ""),
    method!("selectNodeContents", "selectNodeContents(node)", ""),
    method!("setEnd", "setEnd(node, offset)", ""),
    method!("setEndAfter", "setEndAfter(node)", ""),
    method!("setEndBefore", "setEndBefore(node)", ""),
    method!("setStart", "setStart(node, offset)", ""),
    method!("setStartAfter", "setStartAfter(node)", ""),
    method!("setStartBefore", "setStartBefore(node)", ""),
    method!("surroundContents", "surroundContents(node)", ""),
    method!("toString", "toString()", "String"),
];

const DOM_EXCEPTION: &[Member] = &[prop!("code", "code", "Number")];

const POINTER_EVENT: &[Member] = &[
    prop!("height", "height", "Number"),
    prop!("isPrimary", "isPrimary", "Boolean"),
    prop!("pointerId", "pointerId", "Number"),
    prop!("pointerType", "pointerType", "String"),
    prop!("pressure", "pressure", "Number"),
    prop!("tangentialPressure", "tangentialPressure", "Number"),
    prop!("tiltX", "tiltX", "Number"),
    prop!("tiltY", "tiltY", "Number"),
    prop!("twist", "twist", "Number"),
    prop!("width", "width", "Number"),
];

const WHEEL_EVENT: &[Member] = &[
    prop!("deltaMode", "deltaMode", "Number"),
    prop!("deltaX", "deltaX", "Number"),
    prop!("deltaY", "deltaY", "Number"),
    prop!("deltaZ", "deltaZ", "Number"),
];

const DATA_TRANSFER: &[Member] = &[
    prop!("dropEffect", "dropEffect", "String"),
    prop!("effectAllowed", "effectAllowed", "String"),
    prop!("files", "files", "FileList"),
    prop!("types", "types", "Array"),
    method!("clearData", "clearData(format)", ""),
    method!("getData", "getData(format)", "String"),
    method!("setData", "setData(format, data)", ""),
];

const DRAG_EVENT: &[Member] = &[prop!("dataTransfer", "dataTransfer", "DataTransfer")];
const SUBMIT_EVENT: &[Member] = &[prop!("submitter", "submitter", "HTMLElement")];
const POP_STATE_EVENT: &[Member] = &[prop!("state", "state", "")];
const STORAGE_EVENT: &[Member] = &[
    prop!("key", "key", "String"),
    prop!("newValue", "newValue", "String"),
    prop!("oldValue", "oldValue", "String"),
    prop!("storageArea", "storageArea", "Storage"),
    prop!("url", "url", "String"),
];
const MESSAGE_EVENT: &[Member] = &[
    prop!("data", "data", ""),
    prop!("lastEventId", "lastEventId", "String"),
    prop!("origin", "origin", "String"),
    prop!("ports", "ports", "Array"),
    prop!("source", "source", "Window"),
];
const ERROR_EVENT: &[Member] = &[
    prop!("colno", "colno", "Number"),
    prop!("error", "error", ""),
    prop!("filename", "filename", "String"),
    prop!("lineno", "lineno", "Number"),
    prop!("message", "message", "String"),
];
const PROGRESS_EVENT: &[Member] = &[
    prop!("lengthComputable", "lengthComputable", "Boolean"),
    prop!("loaded", "loaded", "Number"),
    prop!("total", "total", "Number"),
];

const READABLE_STREAM: &[Member] = &[
    prop!("locked", "locked", "Boolean"),
    method!("cancel", "cancel(reason)", "Promise"),
    method!("getReader", "getReader(options)", "Object"),
    method!(
        "pipeThrough",
        "pipeThrough(transform, options)",
        "ReadableStream"
    ),
    method!("pipeTo", "pipeTo(destination, options)", "Promise"),
    method!("tee", "tee()", "Array"),
];

const CRYPTO_KEY: &[Member] = &[
    prop!("algorithm", "algorithm", "Object"),
    prop!("extractable", "extractable", "Boolean"),
    prop!("type", "type", "String"),
    prop!("usages", "usages", "Array"),
];

const SCREEN_INSTANCE: &[Member] = &[
    prop!("availHeight", "availHeight", "Number"),
    prop!("availLeft", "availLeft", "Number"),
    prop!("availTop", "availTop", "Number"),
    prop!("availWidth", "availWidth", "Number"),
    prop!("colorDepth", "colorDepth", "Number"),
    prop!("height", "height", "Number"),
    prop!("pixelDepth", "pixelDepth", "Number"),
    prop!("width", "width", "Number"),
];

const VISUAL_VIEWPORT: &[Member] = &[
    prop!("height", "height", "Number"),
    prop!("offsetLeft", "offsetLeft", "Number"),
    prop!("offsetTop", "offsetTop", "Number"),
    prop!("pageLeft", "pageLeft", "Number"),
    prop!("pageTop", "pageTop", "Number"),
    prop!("scale", "scale", "Number"),
    prop!("width", "width", "Number"),
];

const IMAGE_INSTANCE: &[Member] = &[
    prop!("complete", "complete", "Boolean"),
    prop!("naturalHeight", "naturalHeight", "Number"),
    prop!("naturalWidth", "naturalWidth", "Number"),
];

const TYPES: &[Type] = &[
    Type {
        name: "Object",
        documentation: "Root of the JavaScript object hierarchy.",
        constructable: true,
        namespace: false,
        instance: OBJECT_INSTANCE,
        static_members: OBJECT_STATIC,
    },
    Type {
        name: "Array",
        documentation: "Ordered list of values.",
        constructable: true,
        namespace: false,
        instance: ARRAY_INSTANCE,
        static_members: ARRAY_STATIC,
    },
    Type {
        name: "String",
        documentation: "Sequence of UTF-16 code units.",
        constructable: true,
        namespace: false,
        instance: STRING_INSTANCE,
        static_members: STRING_STATIC,
    },
    Type {
        name: "Number",
        documentation: "IEEE-754 floating-point number.",
        constructable: true,
        namespace: false,
        instance: NUMBER_INSTANCE,
        static_members: NUMBER_STATIC,
    },
    Type {
        name: "Boolean",
        documentation: "true or false.",
        constructable: true,
        namespace: false,
        instance: BOOLEAN_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Symbol",
        documentation: "Unique primitive identifier.",
        constructable: true,
        namespace: false,
        instance: SYMBOL_INSTANCE,
        static_members: SYMBOL_STATIC,
    },
    Type {
        name: "BigInt",
        documentation: "Arbitrary-precision integer.",
        constructable: true,
        namespace: false,
        instance: BIGINT_INSTANCE,
        static_members: BIGINT_STATIC,
    },
    Type {
        name: "Function",
        documentation: "Callable object.",
        constructable: true,
        namespace: false,
        instance: FUNCTION_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Date",
        documentation: "Instant in time.",
        constructable: true,
        namespace: false,
        instance: DATE_INSTANCE,
        static_members: DATE_STATIC,
    },
    Type {
        name: "RegExp",
        documentation: "Regular expression.",
        constructable: true,
        namespace: false,
        instance: REGEXP_INSTANCE,
        static_members: REGEXP_STATIC,
    },
    Type {
        name: "Error",
        documentation: "Generic exception.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: ERROR_STATIC,
    },
    Type {
        name: "TypeError",
        documentation: "Value is not of the expected type.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "RangeError",
        documentation: "Value is outside the allowed range.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "ReferenceError",
        documentation: "Invalid reference.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "SyntaxError",
        documentation: "Source text could not be parsed.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "URIError",
        documentation: "Malformed URI.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "EvalError",
        documentation: "Error from the global eval function.",
        constructable: true,
        namespace: false,
        instance: ERROR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "AggregateError",
        documentation: "Several errors wrapped as one.",
        constructable: true,
        namespace: false,
        instance: AGGREGATE_ERROR,
        static_members: &[],
    },
    Type {
        name: "Map",
        documentation: "Keyed collection of values.",
        constructable: true,
        namespace: false,
        instance: MAP_INSTANCE,
        static_members: MAP_STATIC,
    },
    Type {
        name: "Set",
        documentation: "Collection of unique values.",
        constructable: true,
        namespace: false,
        instance: SET_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "WeakMap",
        documentation: "Map with weakly held keys.",
        constructable: true,
        namespace: false,
        instance: WEAK_MAP,
        static_members: &[],
    },
    Type {
        name: "WeakSet",
        documentation: "Set with weakly held values.",
        constructable: true,
        namespace: false,
        instance: WEAK_SET,
        static_members: &[],
    },
    Type {
        name: "Promise",
        documentation: "Result of an asynchronous operation.",
        constructable: true,
        namespace: false,
        instance: PROMISE_INSTANCE,
        static_members: PROMISE_STATIC,
    },
    Type {
        name: "Proxy",
        documentation: "Intercepts operations on a target object.",
        constructable: true,
        namespace: false,
        instance: &[],
        static_members: &[],
    },
    Type {
        name: "Reflect",
        documentation: "Methods for interceptable JavaScript operations.",
        constructable: false,
        namespace: true,
        instance: &[],
        static_members: REFLECT_STATIC,
    },
    Type {
        name: "JSON",
        documentation: "Parse and serialize JSON text.",
        constructable: false,
        namespace: true,
        instance: &[],
        static_members: JSON_STATIC,
    },
    Type {
        name: "Math",
        documentation: "Numeric constants and functions.",
        constructable: false,
        namespace: true,
        instance: &[],
        static_members: MATH_STATIC,
    },
    Type {
        name: "Atomics",
        documentation: "Atomic operations on shared memory.",
        constructable: false,
        namespace: true,
        instance: &[],
        static_members: ATOMICS_STATIC,
    },
    Type {
        name: "Intl",
        documentation: "Constructors for locale-sensitive formatting.",
        constructable: false,
        namespace: true,
        instance: &[],
        static_members: INTL_STATIC,
    },
    Type {
        name: "ArrayBuffer",
        documentation: "Fixed-length raw binary buffer.",
        constructable: true,
        namespace: false,
        instance: BUFFER_INSTANCE,
        static_members: BUFFER_STATIC,
    },
    Type {
        name: "SharedArrayBuffer",
        documentation: "Raw binary buffer that can be shared across agents.",
        constructable: true,
        namespace: false,
        instance: BUFFER_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "DataView",
        documentation: "Reads and writes numbers in an ArrayBuffer.",
        constructable: true,
        namespace: false,
        instance: DATA_VIEW,
        static_members: &[],
    },
    Type {
        name: "Int8Array",
        documentation: "Typed array of 8-bit signed integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Uint8Array",
        documentation: "Typed array of 8-bit unsigned integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Uint8ClampedArray",
        documentation: "Typed array of 8-bit unsigned integers clamped to 0–255.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Int16Array",
        documentation: "Typed array of 16-bit signed integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Uint16Array",
        documentation: "Typed array of 16-bit unsigned integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Int32Array",
        documentation: "Typed array of 32-bit signed integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Uint32Array",
        documentation: "Typed array of 32-bit unsigned integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Float32Array",
        documentation: "Typed array of 32-bit floats.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "Float64Array",
        documentation: "Typed array of 64-bit floats.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "BigInt64Array",
        documentation: "Typed array of 64-bit signed big integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "BigUint64Array",
        documentation: "Typed array of 64-bit unsigned big integers.",
        constructable: true,
        namespace: false,
        instance: TYPED_INSTANCE,
        static_members: TYPED_STATIC,
    },
    Type {
        name: "WeakRef",
        documentation: "Weak reference to an object.",
        constructable: true,
        namespace: false,
        instance: WEAK_REF,
        static_members: &[],
    },
    Type {
        name: "FinalizationRegistry",
        documentation: "Callback when a registered target is garbage-collected.",
        constructable: true,
        namespace: false,
        instance: FINALIZATION,
        static_members: &[],
    },
    Type {
        name: "Generator",
        documentation: "Object produced by a generator function.",
        constructable: false,
        namespace: false,
        instance: GENERATOR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Iterator",
        documentation: "Object with a next method.",
        constructable: false,
        namespace: false,
        instance: GENERATOR_INSTANCE,
        static_members: ITERATOR_STATIC,
    },
    Type {
        name: "console",
        documentation: "Writes diagnostic messages.",
        constructable: false,
        namespace: true,
        instance: &[],
        static_members: CONSOLE_STATIC,
    },
    Type {
        name: "EventTarget",
        documentation: "Object that receives events.",
        constructable: true,
        namespace: false,
        instance: EVENT_TARGET_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Node",
        documentation: "Node in a document tree.",
        constructable: false,
        namespace: false,
        instance: NODE_INSTANCE,
        static_members: NODE_STATIC,
    },
    Type {
        name: "Element",
        documentation: "An element in a document.",
        constructable: false,
        namespace: false,
        instance: ELEMENT_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "HTMLElement",
        documentation: "An element in an HTML document.",
        constructable: false,
        namespace: false,
        instance: HTML_ELEMENT_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Document",
        documentation: "The page loaded in the browser.",
        constructable: false,
        namespace: false,
        instance: DOCUMENT_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "NodeList",
        documentation: "Collection of nodes, including the result of querySelectorAll.",
        constructable: false,
        namespace: false,
        instance: NODE_LIST_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "HTMLCollection",
        documentation: "Live collection of elements.",
        constructable: false,
        namespace: false,
        instance: HTML_COLLECTION_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "DOMTokenList",
        documentation: "Set of space-separated tokens, such as class names.",
        constructable: false,
        namespace: false,
        instance: DOM_TOKEN_LIST_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Text",
        documentation: "A text node.",
        constructable: false,
        namespace: false,
        instance: TEXT_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Window",
        documentation: "A browser window or frame.",
        constructable: false,
        namespace: false,
        instance: WINDOW_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Location",
        documentation: "The URL of the current document.",
        constructable: false,
        namespace: false,
        instance: LOCATION_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Navigator",
        documentation: "The browser and its capabilities.",
        constructable: false,
        namespace: false,
        instance: NAVIGATOR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Storage",
        documentation: "Key-value storage for the origin.",
        constructable: false,
        namespace: false,
        instance: STORAGE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "CSSStyleDeclaration",
        documentation: "CSS declarations of an element.",
        constructable: false,
        namespace: false,
        instance: CSS_STYLE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Event",
        documentation: "An event dispatched to a target.",
        constructable: true,
        namespace: false,
        instance: EVENT_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "URL",
        documentation: "A parsed URL.",
        constructable: true,
        namespace: false,
        instance: URL_INSTANCE,
        static_members: URL_STATIC,
    },
    Type {
        name: "URLSearchParams",
        documentation: "Query-string parameters.",
        constructable: true,
        namespace: false,
        instance: URL_SEARCH_PARAMS,
        static_members: &[],
    },
    Type {
        name: "FormData",
        documentation: "Form fields sent with a request.",
        constructable: true,
        namespace: false,
        instance: FORM_DATA,
        static_members: &[],
    },
    Type {
        name: "Headers",
        documentation: "HTTP headers.",
        constructable: true,
        namespace: false,
        instance: HEADERS_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Request",
        documentation: "An HTTP request.",
        constructable: true,
        namespace: false,
        instance: REQUEST_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Response",
        documentation: "An HTTP response.",
        constructable: true,
        namespace: false,
        instance: RESPONSE_INSTANCE,
        static_members: RESPONSE_STATIC,
    },
    Type {
        name: "AbortController",
        documentation: "Cancels a fetch or other async operation.",
        constructable: true,
        namespace: false,
        instance: ABORT_CONTROLLER,
        static_members: &[],
    },
    Type {
        name: "AbortSignal",
        documentation: "Signal that an operation was aborted.",
        constructable: false,
        namespace: false,
        instance: ABORT_SIGNAL,
        static_members: ABORT_SIGNAL_STATIC,
    },
    Type {
        name: "Blob",
        documentation: "Immutable binary data.",
        constructable: true,
        namespace: false,
        instance: BLOB_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "File",
        documentation: "A file from a form or the file picker.",
        constructable: true,
        namespace: false,
        instance: FILE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "FileReader",
        documentation: "Reads a blob or file.",
        constructable: true,
        namespace: false,
        instance: FILE_READER,
        static_members: &[],
    },
    Type {
        name: "FileList",
        documentation: "Files selected in an input.",
        constructable: false,
        namespace: false,
        instance: FILE_LIST,
        static_members: &[],
    },
    Type {
        name: "TextEncoder",
        documentation: "Encodes a string as UTF-8 bytes.",
        constructable: true,
        namespace: false,
        instance: TEXT_ENCODER,
        static_members: &[],
    },
    Type {
        name: "TextDecoder",
        documentation: "Decodes bytes into a string.",
        constructable: true,
        namespace: false,
        instance: TEXT_DECODER,
        static_members: &[],
    },
    Type {
        name: "DOMParser",
        documentation: "Parses HTML or XML text into a document.",
        constructable: true,
        namespace: false,
        instance: DOM_PARSER,
        static_members: &[],
    },
    Type {
        name: "XMLHttpRequest",
        documentation: "Sends an HTTP request.",
        constructable: true,
        namespace: false,
        instance: XHR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "WebSocket",
        documentation: "A two-way network socket.",
        constructable: true,
        namespace: false,
        instance: WEB_SOCKET,
        static_members: &[],
    },
    Type {
        name: "MutationObserver",
        documentation: "Reports changes in a DOM tree.",
        constructable: true,
        namespace: false,
        instance: MUTATION_OBSERVER,
        static_members: &[],
    },
    Type {
        name: "IntersectionObserver",
        documentation: "Reports when an element enters a viewport.",
        constructable: true,
        namespace: false,
        instance: INTERSECTION_OBSERVER,
        static_members: &[],
    },
    Type {
        name: "ResizeObserver",
        documentation: "Reports element size changes.",
        constructable: true,
        namespace: false,
        instance: RESIZE_OBSERVER,
        static_members: &[],
    },
    Type {
        name: "CustomEvent",
        documentation: "An event with a custom detail payload.",
        constructable: true,
        namespace: false,
        instance: CUSTOM_EVENT,
        static_members: &[],
    },
    Type {
        name: "MouseEvent",
        documentation: "A mouse or pointer click event.",
        constructable: true,
        namespace: false,
        instance: MOUSE_EVENT,
        static_members: &[],
    },
    Type {
        name: "KeyboardEvent",
        documentation: "A keyboard event.",
        constructable: true,
        namespace: false,
        instance: KEYBOARD_EVENT,
        static_members: &[],
    },
    Type {
        name: "InputEvent",
        documentation: "Text entered into an editable element.",
        constructable: true,
        namespace: false,
        instance: INPUT_EVENT,
        static_members: &[],
    },
    Type {
        name: "FocusEvent",
        documentation: "Focus moving between elements.",
        constructable: true,
        namespace: false,
        instance: FOCUS_EVENT,
        static_members: &[],
    },
    Type {
        name: "History",
        documentation: "Session history of the window.",
        constructable: false,
        namespace: false,
        instance: HISTORY_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Crypto",
        documentation: "Random values and subtle cryptography.",
        constructable: false,
        namespace: false,
        instance: CRYPTO_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "SubtleCrypto",
        documentation: "Digest, sign and encrypt operations.",
        constructable: false,
        namespace: false,
        instance: SUBTLE_CRYPTO,
        static_members: &[],
    },
    Type {
        name: "Performance",
        documentation: "High-resolution time and marks.",
        constructable: false,
        namespace: false,
        instance: PERFORMANCE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "MediaQueryList",
        documentation: "Result of matchMedia.",
        constructable: false,
        namespace: false,
        instance: MEDIA_QUERY_LIST,
        static_members: &[],
    },
    Type {
        name: "DOMRect",
        documentation: "A rectangle from getBoundingClientRect.",
        constructable: true,
        namespace: false,
        instance: DOM_RECT,
        static_members: &[],
    },
    Type {
        name: "DOMStringMap",
        documentation: "data-* attributes of an element.",
        constructable: false,
        namespace: false,
        instance: &[],
        static_members: &[],
    },
    Type {
        name: "DocumentFragment",
        documentation: "A lightweight document used to build nodes.",
        constructable: true,
        namespace: false,
        instance: DOCUMENT_FRAGMENT,
        static_members: &[],
    },
    Type {
        name: "Selection",
        documentation: "The text selection in the document.",
        constructable: false,
        namespace: false,
        instance: SELECTION_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "CustomElementRegistry",
        documentation: "Registry of custom elements.",
        constructable: false,
        namespace: false,
        instance: CUSTOM_ELEMENTS,
        static_members: &[],
    },
    Type {
        name: "Clipboard",
        documentation: "Read and write the system clipboard.",
        constructable: false,
        namespace: false,
        instance: CLIPBOARD_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Worker",
        documentation: "A background script.",
        constructable: true,
        namespace: false,
        instance: WORKER_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "BroadcastChannel",
        documentation: "Messages between same-origin browsing contexts.",
        constructable: true,
        namespace: false,
        instance: BROADCAST_CHANNEL,
        static_members: &[],
    },
    Type {
        name: "CanvasRenderingContext2D",
        documentation: "Drawing context of a canvas.",
        constructable: false,
        namespace: false,
        instance: CANVAS_CONTEXT,
        static_members: &[],
    },
    Type {
        name: "TextMetrics",
        documentation: "Measured width of canvas text.",
        constructable: false,
        namespace: false,
        instance: TEXT_METRICS,
        static_members: &[],
    },
    Type {
        name: "IDBFactory",
        documentation: "Opens IndexedDB databases.",
        constructable: false,
        namespace: false,
        instance: IDB_FACTORY,
        static_members: &[],
    },
    Type {
        name: "IDBDatabase",
        documentation: "An IndexedDB database.",
        constructable: false,
        namespace: false,
        instance: IDB_DATABASE,
        static_members: &[],
    },
    Type {
        name: "IDBTransaction",
        documentation: "A transaction over object stores.",
        constructable: false,
        namespace: false,
        instance: IDB_TRANSACTION,
        static_members: &[],
    },
    Type {
        name: "IDBObjectStore",
        documentation: "An object store in IndexedDB.",
        constructable: false,
        namespace: false,
        instance: IDB_OBJECT_STORE,
        static_members: &[],
    },
    Type {
        name: "IDBIndex",
        documentation: "An index of an object store.",
        constructable: false,
        namespace: false,
        instance: IDB_INDEX,
        static_members: &[],
    },
    Type {
        name: "IDBRequest",
        documentation: "Result of an IndexedDB operation.",
        constructable: false,
        namespace: false,
        instance: IDB_REQUEST,
        static_members: &[],
    },
    Type {
        name: "IDBCursor",
        documentation: "Cursor over an IndexedDB store or index.",
        constructable: false,
        namespace: false,
        instance: IDB_CURSOR,
        static_members: &[],
    },
    Type {
        name: "Collator",
        documentation: "Language-sensitive string comparison.",
        constructable: true,
        namespace: false,
        instance: COLLATOR_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "DateTimeFormat",
        documentation: "Language-sensitive date and time formatting.",
        constructable: true,
        namespace: false,
        instance: INTL_FORMAT,
        static_members: &[],
    },
    Type {
        name: "NumberFormat",
        documentation: "Language-sensitive number formatting.",
        constructable: true,
        namespace: false,
        instance: INTL_FORMAT,
        static_members: &[],
    },
    Type {
        name: "RelativeTimeFormat",
        documentation: "Language-sensitive relative time.",
        constructable: true,
        namespace: false,
        instance: INTL_FORMAT,
        static_members: &[],
    },
    Type {
        name: "ListFormat",
        documentation: "Language-sensitive list formatting.",
        constructable: true,
        namespace: false,
        instance: INTL_FORMAT,
        static_members: &[],
    },
    Type {
        name: "PluralRules",
        documentation: "Plural category for a number.",
        constructable: true,
        namespace: false,
        instance: PLURAL_RULES,
        static_members: &[],
    },
    Type {
        name: "Locale",
        documentation: "A Unicode locale identifier.",
        constructable: true,
        namespace: false,
        instance: LOCALE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Segmenter",
        documentation: "Splits text into graphemes, words or sentences.",
        constructable: true,
        namespace: false,
        instance: SEGMENTER_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "DisplayNames",
        documentation: "Localized names of languages, regions and scripts.",
        constructable: true,
        namespace: false,
        instance: DISPLAY_NAMES,
        static_members: &[],
    },
    Type {
        name: "Comment",
        documentation: "A comment node.",
        constructable: false,
        namespace: false,
        instance: COMMENT_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Range",
        documentation: "A range of a document.",
        constructable: true,
        namespace: false,
        instance: RANGE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "DOMException",
        documentation: "Exception raised by a web API.",
        constructable: true,
        namespace: false,
        instance: DOM_EXCEPTION,
        static_members: &[],
    },
    Type {
        name: "PointerEvent",
        documentation: "Pointer input such as mouse, pen or touch.",
        constructable: true,
        namespace: false,
        instance: POINTER_EVENT,
        static_members: &[],
    },
    Type {
        name: "WheelEvent",
        documentation: "Mouse-wheel or trackpad scrolling.",
        constructable: true,
        namespace: false,
        instance: WHEEL_EVENT,
        static_members: &[],
    },
    Type {
        name: "DragEvent",
        documentation: "A drag-and-drop operation.",
        constructable: true,
        namespace: false,
        instance: DRAG_EVENT,
        static_members: &[],
    },
    Type {
        name: "DataTransfer",
        documentation: "Data carried by a drag operation.",
        constructable: true,
        namespace: false,
        instance: DATA_TRANSFER,
        static_members: &[],
    },
    Type {
        name: "SubmitEvent",
        documentation: "A form submission.",
        constructable: true,
        namespace: false,
        instance: SUBMIT_EVENT,
        static_members: &[],
    },
    Type {
        name: "PopStateEvent",
        documentation: "A session-history navigation.",
        constructable: true,
        namespace: false,
        instance: POP_STATE_EVENT,
        static_members: &[],
    },
    Type {
        name: "StorageEvent",
        documentation: "A change in local or session storage.",
        constructable: true,
        namespace: false,
        instance: STORAGE_EVENT,
        static_members: &[],
    },
    Type {
        name: "MessageEvent",
        documentation: "A message from another browsing context.",
        constructable: true,
        namespace: false,
        instance: MESSAGE_EVENT,
        static_members: &[],
    },
    Type {
        name: "ErrorEvent",
        documentation: "An error reported to a window or worker.",
        constructable: true,
        namespace: false,
        instance: ERROR_EVENT,
        static_members: &[],
    },
    Type {
        name: "ProgressEvent",
        documentation: "Progress of a network or file operation.",
        constructable: true,
        namespace: false,
        instance: PROGRESS_EVENT,
        static_members: &[],
    },
    Type {
        name: "ReadableStream",
        documentation: "A stream of readable chunks.",
        constructable: true,
        namespace: false,
        instance: READABLE_STREAM,
        static_members: &[],
    },
    Type {
        name: "CryptoKey",
        documentation: "A key used by subtle cryptography.",
        constructable: false,
        namespace: false,
        instance: CRYPTO_KEY,
        static_members: &[],
    },
    Type {
        name: "Screen",
        documentation: "The screen showing the window.",
        constructable: false,
        namespace: false,
        instance: SCREEN_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "VisualViewport",
        documentation: "The visual viewport of the window.",
        constructable: false,
        namespace: false,
        instance: VISUAL_VIEWPORT,
        static_members: &[],
    },
    Type {
        name: "Image",
        documentation: "An HTML image element created by the Image constructor.",
        constructable: true,
        namespace: false,
        instance: IMAGE_INSTANCE,
        static_members: &[],
    },
    Type {
        name: "Audio",
        documentation: "An HTML audio element created by the Audio constructor.",
        constructable: true,
        namespace: false,
        instance: &[],
        static_members: &[],
    },
];

const GLOBALS: &[GlobalFn] = &[
    GlobalFn {
        name: "parseInt",
        signature: "parseInt(string, radix)",
        documentation: "Parses a string and returns an integer.",
        returns: "Number",
    },
    GlobalFn {
        name: "parseFloat",
        signature: "parseFloat(string)",
        documentation: "Parses a string and returns a floating-point number.",
        returns: "Number",
    },
    GlobalFn {
        name: "isNaN",
        signature: "isNaN(value)",
        documentation: "True when the coerced value is NaN.",
        returns: "Boolean",
    },
    GlobalFn {
        name: "isFinite",
        signature: "isFinite(value)",
        documentation: "True when the coerced value is a finite number.",
        returns: "Boolean",
    },
    GlobalFn {
        name: "eval",
        signature: "eval(code)",
        documentation: "Evaluates a string as JavaScript.",
        returns: "",
    },
    GlobalFn {
        name: "encodeURI",
        signature: "encodeURI(uri)",
        documentation: "Encodes a URI, keeping URI syntax characters.",
        returns: "String",
    },
    GlobalFn {
        name: "decodeURI",
        signature: "decodeURI(uri)",
        documentation: "Decodes a URI encoded by encodeURI.",
        returns: "String",
    },
    GlobalFn {
        name: "encodeURIComponent",
        signature: "encodeURIComponent(component)",
        documentation: "Encodes a URI component.",
        returns: "String",
    },
    GlobalFn {
        name: "decodeURIComponent",
        signature: "decodeURIComponent(component)",
        documentation: "Decodes a URI component.",
        returns: "String",
    },
    GlobalFn {
        name: "structuredClone",
        signature: "structuredClone(value, options)",
        documentation: "Deep-clones a value using the structured clone algorithm.",
        returns: "",
    },
    GlobalFn {
        name: "queueMicrotask",
        signature: "queueMicrotask(callback)",
        documentation: "Queues a microtask.",
        returns: "",
    },
    GlobalFn {
        name: "setTimeout",
        signature: "setTimeout(handler, timeout, ...arguments)",
        documentation: "Runs a function after a delay and returns a timer handle.",
        returns: "Number",
    },
    GlobalFn {
        name: "clearTimeout",
        signature: "clearTimeout(handle)",
        documentation: "Cancels a timeout created by setTimeout.",
        returns: "",
    },
    GlobalFn {
        name: "setInterval",
        signature: "setInterval(handler, timeout, ...arguments)",
        documentation: "Runs a function repeatedly and returns a timer handle.",
        returns: "Number",
    },
    GlobalFn {
        name: "clearInterval",
        signature: "clearInterval(handle)",
        documentation: "Cancels an interval created by setInterval.",
        returns: "",
    },
    GlobalFn {
        name: "requestAnimationFrame",
        signature: "requestAnimationFrame(callback)",
        documentation: "Runs a callback before the next repaint.",
        returns: "Number",
    },
    GlobalFn {
        name: "cancelAnimationFrame",
        signature: "cancelAnimationFrame(handle)",
        documentation: "Cancels a frame requested with requestAnimationFrame.",
        returns: "",
    },
    GlobalFn {
        name: "fetch",
        signature: "fetch(input, init)",
        documentation: "Starts a network request and returns a promise.",
        returns: "Promise",
    },
    GlobalFn {
        name: "alert",
        signature: "alert(message)",
        documentation: "Shows a message dialog.",
        returns: "",
    },
    GlobalFn {
        name: "confirm",
        signature: "confirm(message)",
        documentation: "Shows a confirmation dialog.",
        returns: "Boolean",
    },
    GlobalFn {
        name: "prompt",
        signature: "prompt(message, defaultValue)",
        documentation: "Shows a text prompt.",
        returns: "String",
    },
    GlobalFn {
        name: "atob",
        signature: "atob(data)",
        documentation: "Decodes a Base64 string.",
        returns: "String",
    },
    GlobalFn {
        name: "btoa",
        signature: "btoa(data)",
        documentation: "Encodes a string as Base64.",
        returns: "String",
    },
    GlobalFn {
        name: "getComputedStyle",
        signature: "getComputedStyle(element, pseudoElt)",
        documentation: "Returns the computed CSS of an element.",
        returns: "CSSStyleDeclaration",
    },
    GlobalFn {
        name: "getSelection",
        signature: "getSelection()",
        documentation: "Returns the current text selection.",
        returns: "Selection",
    },
    GlobalFn {
        name: "matchMedia",
        signature: "matchMedia(query)",
        documentation: "Tests a CSS media query.",
        returns: "MediaQueryList",
    },
    GlobalFn {
        name: "reportError",
        signature: "reportError(error)",
        documentation: "Reports an uncaught exception.",
        returns: "",
    },
    GlobalFn {
        name: "requestIdleCallback",
        signature: "requestIdleCallback(callback, options)",
        documentation: "Runs a callback when the browser is idle.",
        returns: "Number",
    },
    GlobalFn {
        name: "cancelIdleCallback",
        signature: "cancelIdleCallback(handle)",
        documentation: "Cancels an idle callback.",
        returns: "",
    },
];

const BINDINGS: &[GlobalBinding] = &[
    GlobalBinding {
        name: "document",
        type_name: "Document",
        documentation: "The document shown in the current window.",
    },
    GlobalBinding {
        name: "window",
        type_name: "Window",
        documentation: "The current browser window.",
    },
    GlobalBinding {
        name: "navigator",
        type_name: "Navigator",
        documentation: "The browser running this page.",
    },
    GlobalBinding {
        name: "location",
        type_name: "Location",
        documentation: "The URL of the current document.",
    },
    GlobalBinding {
        name: "localStorage",
        type_name: "Storage",
        documentation: "Storage that persists for the origin.",
    },
    GlobalBinding {
        name: "sessionStorage",
        type_name: "Storage",
        documentation: "Storage that lasts for the page session.",
    },
    GlobalBinding {
        name: "history",
        type_name: "History",
        documentation: "Session history of this window.",
    },
    GlobalBinding {
        name: "crypto",
        type_name: "Crypto",
        documentation: "Random values and subtle cryptography.",
    },
    GlobalBinding {
        name: "performance",
        type_name: "Performance",
        documentation: "High-resolution time and marks.",
    },
    GlobalBinding {
        name: "customElements",
        type_name: "CustomElementRegistry",
        documentation: "Registry of custom elements.",
    },
    GlobalBinding {
        name: "indexedDB",
        type_name: "IDBFactory",
        documentation: "Opens IndexedDB databases.",
    },
    GlobalBinding {
        name: "globalThis",
        type_name: "Window",
        documentation: "The global object. In a page script it is the window.",
    },
    GlobalBinding {
        name: "self",
        type_name: "Window",
        documentation: "The current global object. In a page script it is the window.",
    },
    GlobalBinding {
        name: "screen",
        type_name: "Screen",
        documentation: "The screen showing this window.",
    },
];

const CONSTANTS: &[(&str, &str, &str)] = &[
    (
        "NaN",
        "Not-a-number value.",
        "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/NaN",
    ),
    (
        "Infinity",
        "Positive infinity.",
        "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Infinity",
    ),
    (
        "undefined",
        "Primitive value of an uninitialized variable.",
        "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/undefined",
    ),
];

const CONSTRUCTS: &[Construct] = &[
    Construct {
        name: "if",
        insert: "if ()",
        caret: Some(4),
        documentation: "Conditional statement.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/if...else",
    },
    Construct {
        name: "for",
        insert: "for ()",
        caret: Some(5),
        documentation: "Loop statement.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for",
    },
    Construct {
        name: "while",
        insert: "while ()",
        caret: Some(7),
        documentation: "Loop while a condition is true.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/while",
    },
    Construct {
        name: "switch",
        insert: "switch ()",
        caret: Some(8),
        documentation: "Multi-way branch.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/switch",
    },
    Construct {
        name: "try",
        insert: "try {}",
        caret: Some(5),
        documentation: "Handles exceptions.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/try...catch",
    },
    Construct {
        name: "catch",
        insert: "catch ()",
        caret: Some(7),
        documentation: "Handles a thrown exception.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/try...catch",
    },
    Construct {
        name: "function",
        insert: "function ()",
        caret: Some(10),
        documentation: "Declares a function.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/function",
    },
    Construct {
        name: "class",
        insert: "class ",
        caret: None,
        documentation: "Declares a class.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/class",
    },
    Construct {
        name: "const",
        insert: "const ",
        caret: None,
        documentation: "Block-scoped constant binding.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/const",
    },
    Construct {
        name: "let",
        insert: "let ",
        caret: None,
        documentation: "Block-scoped binding.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/let",
    },
    Construct {
        name: "async",
        insert: "async ",
        caret: None,
        documentation: "Marks a function as asynchronous.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/async_function",
    },
    Construct {
        name: "await",
        insert: "await ",
        caret: None,
        documentation: "Waits for a promise.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/await",
    },
    Construct {
        name: "break",
        insert: "break",
        caret: None,
        documentation: "Leaves a loop or switch.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/break",
    },
    Construct {
        name: "case",
        insert: "case ",
        caret: None,
        documentation: "One branch of a switch.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/switch",
    },
    Construct {
        name: "continue",
        insert: "continue",
        caret: None,
        documentation: "Goes to the next loop iteration.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/continue",
    },
    Construct {
        name: "debugger",
        insert: "debugger",
        caret: None,
        documentation: "Invokes an available debugger.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/debugger",
    },
    Construct {
        name: "default",
        insert: "default",
        caret: None,
        documentation: "Fallback switch branch or export.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/switch",
    },
    Construct {
        name: "delete",
        insert: "delete ",
        caret: None,
        documentation: "Removes an object property.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/delete",
    },
    Construct {
        name: "do",
        insert: "do {} while ()",
        caret: Some(4),
        documentation: "Loop that runs at least once.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/do...while",
    },
    Construct {
        name: "else",
        insert: "else ",
        caret: None,
        documentation: "Alternative branch of if.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/if...else",
    },
    Construct {
        name: "export",
        insert: "export ",
        caret: None,
        documentation: "Exports a binding from a module.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/export",
    },
    Construct {
        name: "extends",
        insert: "extends ",
        caret: None,
        documentation: "Sets the parent of a class.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Classes/extends",
    },
    Construct {
        name: "finally",
        insert: "finally {}",
        caret: Some(9),
        documentation: "Runs after try/catch.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/try...catch",
    },
    Construct {
        name: "import",
        insert: "import ",
        caret: None,
        documentation: "Imports a binding from a module.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/import",
    },
    Construct {
        name: "in",
        insert: "in ",
        caret: None,
        documentation: "True when a property is on an object.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/in",
    },
    Construct {
        name: "instanceof",
        insert: "instanceof ",
        caret: None,
        documentation: "True when an object is an instance of a constructor.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/instanceof",
    },
    Construct {
        name: "new",
        insert: "new ",
        caret: None,
        documentation: "Creates an instance of a constructor.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/new",
    },
    Construct {
        name: "of",
        insert: "of ",
        caret: None,
        documentation: "Iterates values in for...of.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for...of",
    },
    Construct {
        name: "return",
        insert: "return ",
        caret: None,
        documentation: "Returns a value from a function.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/return",
    },
    Construct {
        name: "static",
        insert: "static ",
        caret: None,
        documentation: "Declares a static class member.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Classes/static",
    },
    Construct {
        name: "super",
        insert: "super",
        caret: None,
        documentation: "Refers to the parent constructor or prototype.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/super",
    },
    Construct {
        name: "this",
        insert: "this",
        caret: None,
        documentation: "The receiver of the current call.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/this",
    },
    Construct {
        name: "throw",
        insert: "throw ",
        caret: None,
        documentation: "Raises an exception.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/throw",
    },
    Construct {
        name: "typeof",
        insert: "typeof ",
        caret: None,
        documentation: "Returns the type tag of a value.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/typeof",
    },
    Construct {
        name: "var",
        insert: "var ",
        caret: None,
        documentation: "Function-scoped binding.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/var",
    },
    Construct {
        name: "void",
        insert: "void ",
        caret: None,
        documentation: "Evaluates an expression and returns undefined.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/void",
    },
    Construct {
        name: "yield",
        insert: "yield ",
        caret: None,
        documentation: "Pauses a generator.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/yield",
    },
    Construct {
        name: "null",
        insert: "null",
        caret: None,
        documentation: "Intentional absence of an object value.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/null",
    },
    Construct {
        name: "true",
        insert: "true",
        caret: None,
        documentation: "Boolean true.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Boolean",
    },
    Construct {
        name: "false",
        insert: "false",
        caret: None,
        documentation: "Boolean false.",
        url: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Boolean",
    },
];

/// Built-in types in completion order.
#[must_use]
pub const fn types() -> &'static [Type] {
    TYPES
}

/// Global functions.
#[must_use]
pub const fn global_functions() -> &'static [GlobalFn] {
    GLOBALS
}

/// Global constant names and their documentation URLs.
#[must_use]
pub const fn global_constants() -> &'static [(&'static str, &'static str, &'static str)] {
    CONSTANTS
}

/// Global object bindings such as `document` and `window`.
#[must_use]
pub const fn global_bindings() -> &'static [GlobalBinding] {
    BINDINGS
}

/// Finds a global object binding by its exact name.
#[must_use]
pub fn global_binding(name: &str) -> Option<&'static GlobalBinding> {
    BINDINGS.iter().find(|binding| binding.name == name)
}

/// DOM event interface for a listener type such as `"click"`. Unknown names stay `Event`.
#[must_use]
pub fn event_type(name: &str) -> &'static str {
    match name {
        "click" | "dblclick" | "mousedown" | "mouseup" | "mousemove" | "mouseenter"
        | "mouseleave" | "mouseover" | "mouseout" | "contextmenu" | "auxclick" => "MouseEvent",
        "pointerdown" | "pointerup" | "pointermove" | "pointerenter" | "pointerleave"
        | "pointerover" | "pointerout" | "pointercancel" | "gotpointercapture"
        | "lostpointercapture" => "PointerEvent",
        "keydown" | "keyup" | "keypress" => "KeyboardEvent",
        "input" | "beforeinput" => "InputEvent",
        "focus" | "blur" | "focusin" | "focusout" => "FocusEvent",
        "wheel" => "WheelEvent",
        "drag" | "dragstart" | "dragend" | "dragenter" | "dragleave" | "dragover" | "drop" => {
            "DragEvent"
        }
        "submit" => "SubmitEvent",
        "popstate" => "PopStateEvent",
        "storage" => "StorageEvent",
        "message" | "messageerror" => "MessageEvent",
        "error" => "ErrorEvent",
        "progress" | "loadstart" | "loadend" | "timeout" => "ProgressEvent",
        _ => "Event",
    }
}

/// Value a promise method fulfills with. Empty when the fulfillment is not a known type.
#[must_use]
pub fn promise_result(type_name: &str, method: &str) -> &'static str {
    match (type_name, method) {
        ("Response" | "Request" | "Blob", "text") | ("Clipboard", "readText") => "String",
        ("Response" | "Request" | "Blob", "arrayBuffer")
        | ("SubtleCrypto", "digest" | "encrypt" | "decrypt" | "sign" | "deriveBits") => {
            "ArrayBuffer"
        }
        ("Response" | "Request", "blob") => "Blob",
        ("Response" | "Request", "formData") => "FormData",
        ("Blob", "bytes") => "Uint8Array",
        ("SubtleCrypto", "generateKey" | "importKey" | "deriveKey") => "CryptoKey",
        ("SubtleCrypto", "verify") => "Boolean",
        ("IDBFactory", "databases") => "Array",
        ("CustomElementRegistry", "whenDefined") => "Function",
        _ => "",
    }
}

/// Value a global function's promise fulfills with. Empty when it is not known.
#[must_use]
pub fn global_promise(name: &str) -> &'static str {
    match name {
        "fetch" => "Response",
        _ => "",
    }
}

/// Element type of a collection. Empty when the items are not a known type.
#[must_use]
pub fn element_of(type_name: &str) -> &'static str {
    match type_name {
        "NodeList" | "HTMLCollection" => "HTMLElement",
        "FileList" => "File",
        "DOMTokenList" => "String",
        _ => "",
    }
}

/// Type of the first callback argument for a collection method. Empty when unknown.
#[must_use]
pub fn callback_item(type_name: &str, method: &str) -> &'static str {
    match method {
        "forEach" | "map" | "filter" | "find" | "findLast" | "some" | "every" | "flatMap" => {
            element_of(type_name)
        }
        _ => "",
    }
}

fn prototype_of(name: &str) -> Option<&'static str> {
    match name {
        "Document" | "Element" | "Text" | "Comment" | "DocumentFragment" => Some("Node"),
        "HTMLElement" => Some("Element"),
        "Image" | "Audio" => Some("HTMLElement"),
        "File" => Some("Blob"),
        "AggregateError" | "DOMException" => Some("Error"),
        "PointerEvent" | "WheelEvent" | "DragEvent" => Some("MouseEvent"),
        "CustomEvent" | "MouseEvent" | "KeyboardEvent" | "InputEvent" | "FocusEvent"
        | "SubmitEvent" | "PopStateEvent" | "StorageEvent" | "MessageEvent" | "ErrorEvent"
        | "ProgressEvent" => Some("Event"),
        "Node" | "Window" | "MediaQueryList" | "Worker" | "BroadcastChannel" | "AbortSignal"
        | "WebSocket" | "XMLHttpRequest" | "FileReader" | "IDBDatabase" | "IDBTransaction"
        | "IDBRequest" | "VisualViewport" => Some("EventTarget"),
        _ => None,
    }
}

/// Statement and keyword snippets.
#[must_use]
pub const fn constructs() -> &'static [Construct] {
    CONSTRUCTS
}

/// Finds a built-in type by its exact name.
#[must_use]
pub fn type_named(name: &str) -> Option<&'static Type> {
    TYPES.iter().find(|ty| ty.name == name)
}

/// Finds a global function by its exact name.
#[must_use]
pub fn global_fn(name: &str) -> Option<&'static GlobalFn> {
    GLOBALS.iter().find(|fun| fun.name == name)
}

/// Members visible on a receiver. Instance lookup walks the host prototype
/// chain and also sees `Object.prototype`.
#[must_use]
pub fn members(type_name: &str, static_access: bool) -> Vec<&'static Member> {
    let Some(ty) = type_named(type_name) else {
        return Vec::new();
    };
    let mut list: Vec<_> = if static_access || ty.namespace {
        ty.static_members.iter().collect()
    } else {
        inherited_instance(type_name)
    };
    if !static_access && !ty.namespace && type_name != "Object" {
        for member in OBJECT_INSTANCE {
            if !list.iter().any(|existing| existing.name == member.name) {
                list.push(member);
            }
        }
    }
    list
}

fn inherited_instance(type_name: &str) -> Vec<&'static Member> {
    let mut list: Vec<&'static Member> = Vec::new();
    let mut next = Some(type_name);
    let mut guard = 0_u8;
    while let Some(name) = next {
        if guard > 8 {
            break;
        }
        guard += 1;
        if let Some(ty) = type_named(name) {
            for member in ty.instance {
                if !list.iter().any(|existing| existing.name == member.name) {
                    list.push(member);
                }
            }
        }
        next = prototype_of(name);
    }
    list
}

/// One member of a built-in, including `Object.prototype` for instances.
#[must_use]
pub fn member(type_name: &str, name: &str, static_access: bool) -> Option<&'static Member> {
    members(type_name, static_access)
        .into_iter()
        .find(|member| member.name == name)
}

const WEB_API: &[&str] = &[
    "AbortController",
    "AbortSignal",
    "Audio",
    "Blob",
    "BroadcastChannel",
    "CSSStyleDeclaration",
    "CanvasRenderingContext2D",
    "Clipboard",
    "Comment",
    "Crypto",
    "CryptoKey",
    "CustomElementRegistry",
    "CustomEvent",
    "DOMException",
    "DOMParser",
    "DOMRect",
    "DOMStringMap",
    "DOMTokenList",
    "DataTransfer",
    "Document",
    "DocumentFragment",
    "DragEvent",
    "Element",
    "ErrorEvent",
    "Event",
    "EventTarget",
    "File",
    "FileList",
    "FileReader",
    "FocusEvent",
    "FormData",
    "HTMLCollection",
    "HTMLElement",
    "Headers",
    "History",
    "IDBCursor",
    "IDBDatabase",
    "IDBFactory",
    "IDBIndex",
    "IDBObjectStore",
    "IDBRequest",
    "IDBTransaction",
    "Image",
    "InputEvent",
    "IntersectionObserver",
    "KeyboardEvent",
    "Location",
    "MediaQueryList",
    "MessageEvent",
    "MouseEvent",
    "MutationObserver",
    "Navigator",
    "Node",
    "NodeList",
    "Performance",
    "PointerEvent",
    "PopStateEvent",
    "ProgressEvent",
    "Range",
    "ReadableStream",
    "Request",
    "ResizeObserver",
    "Response",
    "Screen",
    "Selection",
    "Storage",
    "StorageEvent",
    "SubmitEvent",
    "SubtleCrypto",
    "Text",
    "TextDecoder",
    "TextEncoder",
    "TextMetrics",
    "URL",
    "URLSearchParams",
    "VisualViewport",
    "WebSocket",
    "WheelEvent",
    "Window",
    "Worker",
    "XMLHttpRequest",
    "console",
];

fn web_api(name: &str) -> bool {
    WEB_API.binary_search(&name).is_ok()
}

fn intl_type(name: &str) -> bool {
    matches!(
        name,
        "Collator"
            | "DateTimeFormat"
            | "DisplayNames"
            | "ListFormat"
            | "Locale"
            | "NumberFormat"
            | "PluralRules"
            | "RelativeTimeFormat"
            | "Segmenter"
    )
}

fn type_url(name: &str) -> String {
    if web_api(name) {
        format!("https://developer.mozilla.org/en-US/docs/Web/API/{name}")
    } else if intl_type(name) {
        format!(
            "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl/{name}"
        )
    } else {
        format!(
            "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/{name}"
        )
    }
}

fn member_url(type_name: &str, member: &str) -> String {
    if web_api(type_name) {
        format!("https://developer.mozilla.org/en-US/docs/Web/API/{type_name}/{member}")
    } else if intl_type(type_name) {
        format!(
            "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl/{type_name}/{member}"
        )
    } else {
        format!(
            "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/{type_name}/{member}"
        )
    }
}

/// Markdown hover for an exact global or keyword. Member names are not resolved here.
#[must_use]
pub fn global_info(symbol: &str) -> Option<SymbolDocInfo> {
    if let Some(binding) = global_binding(symbol) {
        return Some(SymbolDocInfo {
            name: binding.name.into(),
            kind: EntityKind::Constant,
            signature: binding.type_name.into(),
            description: binding.documentation.into(),
            target: Some(DefinitionTarget::ExternalUrl(type_url(binding.type_name))),
        });
    }
    if let Some(ty) = type_named(symbol) {
        return Some(SymbolDocInfo {
            name: ty.name.into(),
            kind: EntityKind::Class,
            signature: ty.name.into(),
            description: ty.documentation.into(),
            target: Some(DefinitionTarget::ExternalUrl(type_url(ty.name))),
        });
    }
    if let Some(fun) = global_fn(symbol) {
        return Some(SymbolDocInfo {
            name: fun.name.into(),
            kind: EntityKind::Function,
            signature: fun.signature.into(),
            description: fun.documentation.into(),
            target: Some(DefinitionTarget::ExternalUrl(type_url(fun.name))),
        });
    }
    if let Some((name, documentation, url)) = CONSTANTS.iter().find(|item| item.0 == symbol) {
        return Some(SymbolDocInfo {
            name: (*name).into(),
            kind: EntityKind::Constant,
            signature: (*name).into(),
            description: (*documentation).into(),
            target: Some(DefinitionTarget::ExternalUrl((*url).into())),
        });
    }
    if let Some(construct) = CONSTRUCTS.iter().find(|item| item.name == symbol) {
        return Some(SymbolDocInfo {
            name: construct.name.into(),
            kind: EntityKind::Keyword,
            signature: construct.name.into(),
            description: construct.documentation.into(),
            target: Some(DefinitionTarget::ExternalUrl(construct.url.into())),
        });
    }
    None
}

/// Hover text for a known built-in member. Ambiguous bare names are ignored.
#[must_use]
pub fn member_info(
    type_name: &str,
    name: &str,
    static_access: bool,
) -> Option<(&'static str, String)> {
    let member = member(type_name, name, static_access)?;
    Some((member.signature, member_url(type_name, name)))
}

/// Documentation provider for exact JavaScript globals and keywords.
#[derive(Debug, Default)]
pub struct JsDocs;

impl LanguageDocProvider for JsDocs {
    fn resolve_symbol_info(&self, language_id: &str, symbol: &str) -> Option<SymbolDocInfo> {
        (language_id == "javascript")
            .then(|| global_info(symbol))
            .flatten()
    }

    fn resolve_doc_url(
        &self,
        language_id: &str,
        symbol: &str,
        _entity_type: EntityKind,
    ) -> Option<String> {
        self.resolve_symbol_info(language_id, symbol)
            .and_then(|info| match info.target {
                Some(DefinitionTarget::ExternalUrl(url)) => Some(url),
                Some(DefinitionTarget::LocalFile { .. }) | None => None,
            })
    }
}

/// Renders a signature and optional prose as a JavaScript hover.
#[must_use]
pub fn hover_markup(signature: &str, documentation: &str) -> lsp_types::Hover {
    let value = if documentation.is_empty() {
        format!("```javascript\n{signature}\n```")
    } else {
        format!("```javascript\n{signature}\n```\n\n{documentation}")
    };
    lsp_types::Hover {
        contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
            kind: lsp_types::MarkupKind::Markdown,
            value,
        }),
        range: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WEB_API, element_of, event_type, global_binding, global_promise, member, members,
        promise_result, type_named, type_url, types,
    };

    fn names_of(type_name: &str) -> Vec<&'static str> {
        members(type_name, false)
            .into_iter()
            .map(|item| item.name)
            .collect()
    }

    #[test]
    fn web_catalog_names_are_sorted_types() {
        for pair in WEB_API.windows(2) {
            assert!(pair[0] < pair[1], "{} is not before {}", pair[0], pair[1]);
        }
        for name in WEB_API {
            assert!(type_named(name).is_some(), "{name}");
            assert!(
                type_url(name).contains("/Web/API/"),
                "{name} -> {}",
                type_url(name)
            );
        }
        assert!(type_url("NumberFormat").contains("/Intl/NumberFormat"));
        let mut type_names: Vec<_> = types().iter().map(|ty| ty.name).collect();
        let count = type_names.len();
        type_names.sort_unstable();
        type_names.dedup();
        assert_eq!(type_names.len(), count);
        for ty in types() {
            for slice in [ty.instance, ty.static_members] {
                let mut seen = Vec::new();
                for item in slice {
                    assert!(
                        !seen.contains(&item.name),
                        "{} lists {} twice",
                        ty.name,
                        item.name
                    );
                    seen.push(item.name);
                }
            }
        }
    }

    #[test]
    fn page_queries_expose_html_element_fields() {
        assert_eq!(
            member("Document", "querySelector", false).map(|item| item.returns),
            Some("HTMLElement")
        );
        assert_eq!(
            member("Document", "querySelectorAll", false).map(|item| item.returns),
            Some("NodeList")
        );
        assert_eq!(element_of("NodeList"), "HTMLElement");
        assert_eq!(element_of("HTMLCollection"), "HTMLElement");
        assert_eq!(element_of("FileList"), "File");
        assert_eq!(element_of("DOMTokenList"), "String");
        let element = names_of("HTMLElement");
        for name in [
            "value",
            "href",
            "style",
            "dataset",
            "addEventListener",
            "querySelector",
            "textContent",
            "classList",
        ] {
            assert!(element.contains(&name), "{name}");
        }
        let pointer = names_of("PointerEvent");
        assert!(pointer.contains(&"pointerId"));
        assert!(pointer.contains(&"clientX"));
        assert!(pointer.contains(&"preventDefault"));
        assert!(names_of("File").contains(&"arrayBuffer"));
        assert!(names_of("CSSStyleDeclaration").contains(&"backgroundColor"));
        assert_eq!(
            global_binding("document").map(|item| item.type_name),
            Some("Document")
        );
        assert_eq!(
            global_binding("globalThis").map(|item| item.type_name),
            Some("Window")
        );
        assert_eq!(event_type("click"), "MouseEvent");
        assert_eq!(event_type("keydown"), "KeyboardEvent");
        assert_eq!(event_type("load"), "Event");
        assert_eq!(global_promise("fetch"), "Response");
        assert_eq!(promise_result("Response", "text"), "String");
        assert_eq!(promise_result("Response", "json"), "");
        assert_eq!(
            member("Intl", "NumberFormat", true).map(|item| item.returns),
            Some("NumberFormat")
        );
        assert!(names_of("NumberFormat").contains(&"format"));
    }
}
