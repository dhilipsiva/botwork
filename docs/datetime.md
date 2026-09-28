# Date/time statements

The default Engine and CLI provide sixteen date/time statements as part of the
97-signature fixed catalogue. Timestamps and durations are Strings; comparisons
return Int `-1`, `0`, or `1`. No Float conversion or machine-local timezone is
implicit. All arguments except `amount` are Strings, checked without coercion.

| Signature | Result |
| --- | --- |
| `Current Date Time In \|zone\|` | Wall clock captured once, in the chosen zone |
| `Parse Date Time \|text\|` | Parse explicit-offset RFC 3339 and normalize to UTC |
| `Parse Date Time \|text\| Using \|format\| In \|zone\| Choosing \|ambiguity\|` | Parse local calendar fields in an explicit zone; normalize to UTC |
| `Format Date Time \|datetime\| Using \|format\| In \|zone\|` | Format an instant in the chosen zone |
| `Convert Date Time \|datetime\| To \|zone\|` | RFC 3339 at the same instant with the chosen offset |
| `Add Duration \|duration\| To Date Time \|datetime\|` | UTC after adding elapsed time |
| `Subtract Duration \|duration\| From Date Time \|datetime\|` | UTC after subtracting elapsed time |
| `Difference Between Date Times \|left\| And \|right\|` | Exact signed duration, left minus right |
| `Compare Date Times \|left\| And \|right\|` | Compare instants, independent of displayed offsets |
| `Parse Duration \|text\|` | Canonical duration |
| `Create Duration \|amount\| In \|unit\|` | Duration from an Int and a plural unit name |
| `Duration Seconds \|duration\|` | Exact signed decimal seconds as a String |
| `Duration Nanoseconds \|duration\|` | Exact signed integer nanoseconds as a String |
| `Add Durations \|left\| And \|right\|` | Canonical sum |
| `Subtract Durations \|left\| Minus \|right\|` | Canonical difference |
| `Compare Durations \|left\| And \|right\|` | Compare exact signed elapsed values |

## Timestamps and zones

Timestamp inputs use `YYYY-MM-DDTHH:MM:SS[.fraction]Z` or the same date/time
followed by `±HH:MM`. Uppercase `T`/`Z`, four-digit years, two-digit fields,
and explicit offsets are required. Fractional seconds have one through nine
ASCII digits. Dates use the proleptic Gregorian calendar, including year zero.
Seconds are POSIX seconds: leap-second inputs fail. Unknown `-00:00` offsets,
omitted offsets, invalid dates, and extra fractional digits fail instead of being
guessed or truncated. Offset hours must be below 24 and minutes below 60.

Canonical output removes trailing fractional zeros and uses `Z` for zero offset.
The represented local and UTC years must both be in 0000–9999. Parsing an offset
date just beyond the UTC boundary fails. Arithmetic returns UTC; it does not
retain the input's offset or a zone identifier.

Zones are `UTC`, `Z`, strict `±HH:MM`, or exact case-sensitive IANA names, such as
`Asia/Kolkata` and `America/New_York`. Neither the environment's `TZ` nor operating
system timezone files are consulted. The lockfile bundles chrono-tz 0.10.4 with
IANA data **2025b**. Named-zone operations require both local and UTC years in
**1800–2099**, matching the recurring-rule expansion horizon of its table
generator; requests outside that range fail. UTC/fixed-offset operations retain
the wider timestamp range. Future predictions reflect this bundled database and
can change after a reviewed dependency update. Historical records depend on the
database's available evidence. See [chrono-tz](https://docs.rs/chrono-tz/0.10.4/chrono_tz/)
and its [transition generator](https://docs.rs/parse-zoneinfo/0.4.0/src/parse_zoneinfo/transitions.rs.html).

Local parsing requires `ambiguity` to be exactly `reject`, `earlier`, or `later`,
even if the time has only one interpretation. During a backward clock change,
the choices select the earlier/later **instant**, including half-hour changes.
`reject` reports the repeated time. Times skipped by a forward change always
fail; no choice shifts them into existence.

Historical named zones can have offsets containing seconds. Local parsing
preserves them when converting to UTC. Current/Convert RFC 3339 output rejects
such offsets because RFC 3339 cannot encode them. Use a format containing `%::z`
to display the full `±HH:MM:SS` offset.

## Calendar formats and wall clock

Formats use [Chrono's strftime syntax](https://docs.rs/chrono/0.4.45/chrono/format/strftime/index.html):
`%F` is a calendar date, `%T` is time, `%.9f` is a decimal point and nine fractional
digits, `%:z` is an offset with minutes, `%::z` includes seconds, `%Z` displays the
zone abbreviation (a numeric offset for fixed zones, including `UTC`/`Z`), and
`%%` is a percent sign. Textual month/day names use English, independent of locale.
Output formats intentionally select precision: omitting fractions loses displayed
precision, and minute-only offset formats cannot show historical offset seconds.
An empty output format returns an empty String. Invalid format directives fail.

Local parsing needs sufficient date/time fields; date-only input does not invent
midnight. It forbids epoch and timezone directives (`%s`, `%z` variants, `%Z`,
`%+`, and RFC shortcuts containing zones), so they cannot conflict with or be
silently ignored in favor of the zone argument. For fractional parsing use
`%.3f`, `%.6f`, or `%.9f` (or `%f` for integer nanoseconds). Variable-precision
`%.f` and internal no-dot fixed-precision variants are unsupported for parsing.
The fixed dotted forms allow an absent fraction; when present they require the
specified digit count. Formatting supports Chrono's broader directives.

Current Date Time reads the operating system wall clock, which can move backward
and is not controlled by a paused Tokio timer. It is unsuitable for measuring
elapsed runtime or deadlines; existing execution controls use their own clocks.

## Exact elapsed durations

Accepted syntax is an optional `+`/`-`, followed by `PnW` **or**
`P[nD][T[nH][nM][n[.fraction]S]]`. At least one unit is required; `T` must introduce
a time unit. Units occur once in that order. Only seconds accept a fraction,
with one through nine digits and a required integer part. Whitespace, lowercase,
calendar months/years, mixed week fields, fractional days/hours/minutes, and
decimal commas fail. Counts can exceed clock ranges: `PT90S` becomes `PT1M30S`.
Leading zeros are allowed. Weeks normalize to days, zero becomes `PT0S`, and
canonical output omits zero units and trailing fractional zeros.

Durations are checked signed 128-bit nanoseconds, from
`-170141183460469231731687303715.884105728` through
`170141183460469231731687303715.884105727` seconds. Arithmetic never wraps.
Create Duration accepts the plural names `weeks`, `days`, `hours`, `minutes`,
`seconds`, `milliseconds`, `microseconds`, and `nanoseconds` with an Int amount.
String totals preserve precision and support values beyond the language's Int
range and the year-2038 boundary. They are decimal values, not Float expressions.

A day is exactly 86400 elapsed seconds. Adding one day across a daylight-saving
change can change the displayed local hour. Calendar month/year arithmetic and
“same local time tomorrow” policies are deliberately outside this contract.

## Errors, limits, and registration

BW3003 identifies wrong kinds, syntax, zones, formats, choices, gaps, or unresolved
repeated times. BW3002 identifies arithmetic/range overflow or an offset that
RFC 3339 cannot represent. BW8001 identifies resource exhaustion. Errors retain
call/source context; ordinary failures can be caught and preserve assignment
destinations. Statements have the same behavior in synchronous and asynchronous
runs. They run inline; parsing is bounded by admitted input sizes, and stop
checks surround library work. Formatting checks stop control per emitted chunk.

Output String bytes and live argument/output overlap are measured before allocating
the result. Formatting streams twice through bounded writers instead of making
an unbounded formatted intermediate. Library calendar values and bundled offset
names are small bounded workspace. Standard execution, values, temporaries,
diagnostics, and result-export budgets still apply.

Initialization is idempotent and preserves earlier host registrations. The fixed
catalogue is exempt from user registry retention budgets; its 97 table slots
count toward snapshot work. These names occupy default root slots; child scopes
can shadow them. Use `--statement-help 'Parse Date Time |text|'` for typed metadata.

<!-- botwork-test: datetime-statements -->
```botwork
|start| = |@{ Parse Date Time |"2024-11-03 01:30:00"| Using |"%F %T"| In |"America/New_York"| Choosing |"earlier"| }|
|end| = |@{ Add Duration |"PT1H"| To Date Time |start| }|
Log |start|
Log |@{ Convert Date Time |end| To |"America/New_York"| }|
Log |@{ Difference Between Date Times |end| And |start| }|
Log |@{ Duration Nanoseconds |"PT0.000000001S"| }|
Log |@{ Format Date Time |end| Using |"%F %T %Z"| In |"America/New_York"| }|
```

Output:

```text
2024-11-03T05:30:00Z
2024-11-03T01:30:00-05:00
PT1H
1
2024-11-03 01:30:00 EST
```

The same program is [example 31](../examples/31-datetime.botwork).
