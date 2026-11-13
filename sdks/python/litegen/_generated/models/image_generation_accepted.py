from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ImageGenerationAccepted")


@_attrs_define
class ImageGenerationAccepted:
    """The answer to an image request that won't wait for its job
    (`Prefer: respond-async`, or a `wait` the job outlasted): litegen
    delivers the image to the generation when the vendor finishes
    (docs/async-images.md).

        Attributes:
            created (int): Unix seconds.
            id (str): The generation: `GET /v1/generations/{id}` serves its image once done.
            model (str):
            object_ (str): Always `image.generation`.
            status (str): `processing`.
    """

    created: int
    id: str
    model: str
    object_: str
    status: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created = self.created

        id = self.id

        model = self.model

        object_ = self.object_

        status = self.status

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "model": model,
                "object": object_,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created = d.pop("created")

        id = d.pop("id")

        model = d.pop("model")

        object_ = d.pop("object")

        status = d.pop("status")

        image_generation_accepted = cls(
            created=created,
            id=id,
            model=model,
            object_=object_,
            status=status,
        )

        image_generation_accepted.additional_properties = d
        return image_generation_accepted

    @property
    def additional_keys(self) -> List[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
