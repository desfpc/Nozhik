<?php
namespace Demo\Model;

class Car extends Vehicle
{
    public function park(): string
    {
        $this->name = 'parked car';
        $this->run();
        return $this->describe();
    }
}
