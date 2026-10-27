<?php
namespace Demo\App;

use Demo\Contracts\Vehicle as VehicleContract;
use Demo\Model\{Car as CityCar, Truck, ElectricCar, Vehicle};

class Fleet
{
    public Vehicle $vehicle;

    public function __construct(Vehicle $vehicle)
    {
        $this->vehicle = $vehicle;
    }

    public function run(VehicleContract $contract): array
    {
        $car = new CityCar();
        $truck = new Truck();
        $electric = new ElectricCar();
        $name = $car->name;
        $car->name = 'city car';
        $car->run();
        $car->park();
        $truck->run();
        $electric->run();
        $electric->charge();
        $this->vehicle->run();
        $instances = Vehicle::$instances;
        $type = Vehicle::TYPE;
        $isCar = $car instanceof CityCar;
        return [$name, $instances, $type, $isCar, $contract->run()];
    }
}
