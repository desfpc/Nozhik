<?php
namespace Demo\Model;

use Demo\Contracts\Vehicle as VehicleContract;

abstract class Vehicle implements VehicleContract
{
    public string $name = 'vehicle';
    public static int $instances = 0;
    public const TYPE = 'vehicle';

    public function run(): string
    {
        self::$instances++;
        return $this->name;
    }

    protected function describe(): string
    {
        return $this->name;
    }

    private function unusedHelper(): void
    {
    }
}
