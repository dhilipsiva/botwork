# U4: Compose results

`pricing.botwork` provides a reusable statement, `Total of |quantity| at |price|`.
Do not change that file. `compose.botwork` already imports it as `pricing`.

Edit `compose.botwork` so that it calculates the total for quantity 3 at price 4
plus the total for quantity 2 at price 7, using the supplied statement for both,
and logs whether that sum equals 26. You may store results in variables along
the way. Do not repeat the multiplication yourself.

Run the script; you are done when it prints `true`.
