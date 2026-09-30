# U3: Reuse a calculation

`reuse.botwork` sets the caller's own `quantity` to 99. Edit it so that it:

1. defines one reusable custom statement that multiplies a quantity by a unit
   price and returns the result;
2. calls that statement with quantity 3 and unit price 4, and logs the result;
3. calls it again with quantity 2 and unit price 7, and logs the result;
4. finally logs the caller's `quantity`.

Write the multiplication only once, inside the statement. Run the script; you
are done when it prints `12`, `14`, and `99`.
