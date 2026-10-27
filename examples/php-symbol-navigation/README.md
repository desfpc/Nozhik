# PHP symbol navigation fixture

Open this directory as a Nozhik project. No Composer install, PHP execution or LSP
is required for the index/navigation checks. The files are a static fixture;
main.php illustrates construction rather than providing an autoloader.

- Contracts/Vehicle.php is an interface implemented by Model/Vehicle.php.
- Car and Truck inherit Vehicle; ElectricCar inherits Car (two levels).
- Car inherits run/name/describe; Truck overrides run and calls parent::run().
- Fleet creates each concrete type, calls own/inherited/overridden methods,
  reads/writes instance/static properties and reads a class constant. It also
  uses a typed property/parameters, grouped imports, an alias and instanceof.
- main.php constructs Fleet and calls its run method.

Cmd (macOS) / Ctrl (Windows/Linux) + hover on a declaration opens its usages list.
Click a row to open the reference and reveal its line. Modifier+click on that
reference returns to the actual declaration (Vehicle.run for inherited calls,
Truck.run for the override). Repeat for classes, properties and methods.
Alt+F7 and the usage-count lens open the same list. Esc closes it.

A class count represents actual type references: imports, inheritance, type hints,
new, instanceof and explicit static scopes. Member calls are counted against the
member declaration, not arbitrarily added to the class count. Self/parent static
scope references are included. Comments and string literals are excluded.

Expected initial counts (asserted by the Rust fixture test):

| Symbol | Usages |
| --- | ---: |
| Demo\Contracts\Vehicle | 4 |
| Demo\Model\Vehicle | 9 |
| Demo\Model\Car | 6 |
| Demo\Model\Truck | 2 |
| Demo\Model\ElectricCar | 2 |
| Demo\App\Fleet | 1 |
| Demo\Model\Vehicle::run | 5 |
| Demo\Model\Truck::run | 1 |
| Demo\Model\Vehicle::$name | 5 |
| Demo\Model\Vehicle::$instances | 2 |
