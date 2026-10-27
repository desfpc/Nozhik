<?php
namespace Demo\Model;

class ElectricCar extends Car
{
    public function charge(): string
    {
        return $this->park();
    }
}
