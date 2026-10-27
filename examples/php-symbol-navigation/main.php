<?php
namespace Demo\App;

use Demo\Model\Car;

function demo(): array
{
    $car = new Car();
    $fleet = new Fleet($car);
    return $fleet->run($car);
}
